use libloading::{Library, Symbol};
use std::ffi::{c_char, c_void};
use std::path::Path;
use std::io::Write;
use thiserror::Error;

pub const RETRO_API_VERSION: u32 = 1;

// Environment callback commands
pub const RETRO_ENVIRONMENT_SET_PIXEL_FORMAT: u32 = 1;
pub const RETRO_ENVIRONMENT_SET_SYSTEM_AV_INFO: u32 = 2;
pub const RETRO_ENVIRONMENT_GET_SYSTEM_DIRECTORY: u32 = 9;
pub const RETRO_ENVIRONMENT_GET_SAVE_DIRECTORY: u32 = 10;
pub const RETRO_ENVIRONMENT_GET_VARIABLE: u32 = 4;
pub const RETRO_ENVIRONMENT_GET_VFS_INTERFACE: u32 = 54;
pub const RETRO_ENVIRONMENT_GET_LOG_INTERFACE: u32 = 11;
pub const RETRO_ENVIRONMENT_GET_CORE_OPTIONS_VERSION: u32 = 46;
pub const RETRO_ENVIRONMENT_SET_CORE_OPTIONS_V2: u32 = 47;
pub const RETRO_ENVIRONMENT_SET_AUDIO_BUFFER_STATUS_CALLBACK: u32 = 17;
pub const RETRO_ENVIRONMENT_GET_LED_INTERFACE: u32 = 36;
pub const RETRO_ENVIRONMENT_SET_INPUT_DESCRIPTORS: u32 = 21;
pub const RETRO_ENVIRONMENT_SET_ROTATION: u32 = 16;
pub const RETRO_ENVIRONMENT_SET_GEOMETRY: u32 = 25;
pub const RETRO_ENVIRONMENT_SET_MESSAGE: u32 = 23;

// Pixel format constants
pub const RETRO_PIXEL_FORMAT_XRGB8888: u32 = 2;

// Input devices
pub const RETRO_DEVICE_JOYPAD: u32 = 1;
pub const RETRO_DEVICE_ID_JOYPAD_B: u32 = 0;
pub const RETRO_DEVICE_ID_JOYPAD_Y: u32 = 1;
pub const RETRO_DEVICE_ID_JOYPAD_SELECT: u32 = 2;
pub const RETRO_DEVICE_ID_JOYPAD_START: u32 = 3;
pub const RETRO_DEVICE_ID_JOYPAD_UP: u32 = 4;
pub const RETRO_DEVICE_ID_JOYPAD_DOWN: u32 = 5;
pub const RETRO_DEVICE_ID_JOYPAD_LEFT: u32 = 6;
pub const RETRO_DEVICE_ID_JOYPAD_RIGHT: u32 = 7;
pub const RETRO_DEVICE_ID_JOYPAD_A: u32 = 8;
pub const RETRO_DEVICE_ID_JOYPAD_X: u32 = 9;
pub const RETRO_DEVICE_ID_JOYPAD_L: u32 = 10;
pub const RETRO_DEVICE_ID_JOYPAD_R: u32 = 11;

#[derive(Debug, Clone)]
pub struct RetroSystemInfo {
    pub library_name: String,
    pub library_version: String,
    pub valid_extensions: String,
    pub need_fullpath: bool,
    pub block_extract: bool,
}

#[derive(Debug, Clone)]
pub struct RetroGameInfo {
    pub path: String,
    pub data: Vec<u8>,
    pub path_cstring: Option<std::ffi::CString>,
}

#[derive(Debug, Clone)]
pub struct RetroSystemAVInfo {
    pub geometry: RetroGameGeometry,
    pub timing: RetroSystemTiming,
}

#[derive(Debug, Clone)]
pub struct RetroGameGeometry {
    pub base_width: u32,
    pub base_height: u32,
    pub max_width: u32,
    pub max_height: u32,
    pub aspect_ratio: f32,
}

#[derive(Debug, Clone)]
pub struct RetroSystemTiming {
    pub fps: f64,
    pub sample_rate: f64,
}

#[derive(Error, Debug)]
pub enum LibretroError {
    #[error("Failed to load core: {0}")]
    LoadFailed(String),
    #[error("API version mismatch")]
    ApiVersionMismatch,
    #[error("Core not loaded")]
    CoreNotLoaded,
    #[error("Failed to load game")]
    GameLoadFailed,
}

pub type RetroEnvironmentFn = extern "C" fn(cmd: u32, data: *mut c_void) -> bool;
pub type RetroVideoRefreshFn = extern "C" fn(data: *const c_void, width: u32, height: u32, pitch: usize);
pub type RetroAudioSampleFn = extern "C" fn(left: i16, right: i16);
pub type RetroAudioSampleBatchFn = extern "C" fn(data: *const i16, frames: usize) -> usize;
pub type RetroInputPollFn = extern "C" fn();
pub type RetroInputStateFn = extern "C" fn(port: u32, device: u32, index: u32, id: u32) -> i16;

pub struct RetroCore {
    library: Library,
}

impl RetroCore {
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self, LibretroError> {
        let lib = unsafe {
            Library::new(path.as_ref()).map_err(|e| LibretroError::LoadFailed(e.to_string()))?
        };

        // Verify API version
        let api_version: Symbol<extern "C" fn() -> u32> = unsafe {
            lib.get(b"retro_api_version")
                .map_err(|_| LibretroError::ApiVersionMismatch)?
        };

        if api_version() != RETRO_API_VERSION {
            return Err(LibretroError::ApiVersionMismatch);
        }

        Ok(RetroCore { library: lib })
    }

    pub fn get_system_info(&self) -> Result<RetroSystemInfo, LibretroError> {
        unsafe {
            let func: Symbol<extern "C" fn(*mut RetroSystemInfoC)> =
                self.library
                    .get(b"retro_get_system_info")
                    .map_err(|_| LibretroError::CoreNotLoaded)?;

            let mut info = RetroSystemInfoC {
                library_name: std::ptr::null(),
                library_version: std::ptr::null(),
                valid_extensions: std::ptr::null(),
                need_fullpath: false,
                block_extract: false,
            };

            func(&mut info);

            Ok(RetroSystemInfo {
                library_name: cstring_to_string(info.library_name),
                library_version: cstring_to_string(info.library_version),
                valid_extensions: cstring_to_string(info.valid_extensions),
                need_fullpath: info.need_fullpath,
                block_extract: info.block_extract,
            })
        }
    }

    pub fn set_callbacks(
        &self,
        env_callback: RetroEnvironmentFn,
        video_callback: RetroVideoRefreshFn,
        input_poll_callback: RetroInputPollFn,
        input_state_callback: RetroInputStateFn,
        audio_callback: RetroAudioSampleFn,
        audio_batch_callback: RetroAudioSampleBatchFn,
    ) -> Result<(), LibretroError> {
        unsafe {
            let set_env: Symbol<extern "C" fn(RetroEnvironmentFn)> = self
                .library
                .get(b"retro_set_environment")
                .map_err(|_| LibretroError::CoreNotLoaded)?;
            set_env(env_callback);

            let set_video: Symbol<extern "C" fn(RetroVideoRefreshFn)> = self
                .library
                .get(b"retro_set_video_refresh")
                .map_err(|_| LibretroError::CoreNotLoaded)?;
            set_video(video_callback);

            let set_audio: Symbol<extern "C" fn(RetroAudioSampleFn)> = self
                .library
                .get(b"retro_set_audio_sample")
                .map_err(|_| LibretroError::CoreNotLoaded)?;
            set_audio(audio_callback);

            // Set batch audio callback (modern cores prefer this)
            if let Ok(set_audio_batch) = self
                .library
                .get::<Symbol<extern "C" fn(RetroAudioSampleBatchFn)>>(b"retro_set_audio_sample_batch")
            {
                set_audio_batch(audio_batch_callback);
            }

            let set_input_poll: Symbol<extern "C" fn(RetroInputPollFn)> = self
                .library
                .get(b"retro_set_input_poll")
                .map_err(|_| LibretroError::CoreNotLoaded)?;
            set_input_poll(input_poll_callback);

            let set_input_state: Symbol<extern "C" fn(RetroInputStateFn)> = self
                .library
                .get(b"retro_set_input_state")
                .map_err(|_| LibretroError::CoreNotLoaded)?;
            set_input_state(input_state_callback);

            Ok(())
        }
    }

    pub fn init(&self) -> Result<(), LibretroError> {
        unsafe {
            let func: Symbol<extern "C" fn()> = self
                .library
                .get(b"retro_init")
                .map_err(|_| LibretroError::CoreNotLoaded)?;
            func();
            Ok(())
        }
    }

    pub fn load_game(&self, game: &RetroGameInfo) -> Result<(), LibretroError> {
        unsafe {
            // Print struct diagnostics
            use memoffset::offset_of;
            
            eprintln!("\n=== RetroGameInfoC Struct Diagnostics ===");
            eprintln!("sizeof(RetroGameInfoC) = {} bytes", std::mem::size_of::<RetroGameInfoC>());
            eprintln!("alignof(RetroGameInfoC) = {} bytes", std::mem::align_of::<RetroGameInfoC>());
            eprintln!("offset_of(path) = {}", offset_of!(RetroGameInfoC, path));
            eprintln!("offset_of(data) = {}", offset_of!(RetroGameInfoC, data));
            eprintln!("offset_of(size) = {}", offset_of!(RetroGameInfoC, size));
            eprintln!("offset_of(meta) = {}", offset_of!(RetroGameInfoC, meta));
            eprintln!("========================================\n");
            
            let func: Symbol<extern "C" fn(*const RetroGameInfoC) -> bool> = self
                .library
                .get(b"retro_load_game")
                .map_err(|_| LibretroError::CoreNotLoaded)?;

            let c_path = game.path_cstring.as_ref()
                .cloned()
                .unwrap_or_else(|| std::ffi::CString::new(game.path.as_str()).unwrap());
            
            // Get pointer to C string data and keep it alive
            let path_ptr = c_path.as_ptr();
            let _leaked_path = Box::leak(Box::new(c_path));
            
            // Don't load ROM data - let cores request it if needed
            // Cores with need_fullpath=false will request data via callbacks if needed
            let rom_data = game.data.clone();
            
            let rom_size = rom_data.len();
            
            // Leak the ROM data to keep it alive
            let data_ptr = if !rom_data.is_empty() {
                Box::leak(Box::new(rom_data)).as_ptr() as *const c_void
            } else {
                std::ptr::null()
            };
            
            // Allocate game_info on the heap and leak it
            let game_info_box = Box::new(RetroGameInfoC {
                path: path_ptr,
                data: data_ptr,
                size: rom_size,
                meta: std::ptr::null(),
            });
            let game_info_ptr = Box::into_raw(game_info_box);
            
            eprintln!("\nCalling retro_load_game()...");
            eprintln!("game_info_ptr = {:p}", game_info_ptr);
            eprintln!("  path = {:p} ({:?})", (*game_info_ptr).path, (*game_info_ptr).path);
            eprintln!("  data = {:p}", (*game_info_ptr).data);
            eprintln!("  size = {}", (*game_info_ptr).size);
            eprintln!("  meta = {:p}", (*game_info_ptr).meta);
            
            eprintln!("\nAbout to call retro_load_game()...");
            eprintln!("Verifying ROM data...");
            unsafe {
                if !(*game_info_ptr).data.is_null() {
                    eprintln!("  Data is not null, reading first bytes...");
                    let first_bytes = std::slice::from_raw_parts(
                        (*game_info_ptr).data as *const u8, 
                        std::cmp::min(16, (*game_info_ptr).size as usize)
                    );
                    eprintln!("  First 16 bytes of ROM: {:?}", first_bytes);
                } else {
                    eprintln!("  Data is null (okay for fullpath cores)");
                }
            }
            eprintln!("About to call func()...");
            let _ = std::io::stderr().flush();
            eprintln!("Calling func() now...");
            let result = if func(game_info_ptr) {
                eprintln!("✅ load_game() returned true");
                let _ = std::io::stderr().flush();
                Ok(())
            } else {
                eprintln!("❌ load_game() returned false");
                let _ = std::io::stderr().flush();
                Err(LibretroError::GameLoadFailed)
            };
            eprintln!("load_game() completed successfully\n");
            let _ = std::io::stderr().flush();
            
            // Don't free - let it leak in case core keeps a reference
            let _ = game_info_ptr;
            
            result
        }
    }

    pub fn run(&self) -> Result<(), LibretroError> {
        unsafe {
            let func: Symbol<extern "C" fn()> = self
                .library
                .get(b"retro_run")
                .map_err(|_| LibretroError::CoreNotLoaded)?;
            func();
            Ok(())
        }
    }

    pub fn unload_game(&self) -> Result<(), LibretroError> {
        unsafe {
            let func: Symbol<extern "C" fn()> = self
                .library
                .get(b"retro_unload_game")
                .map_err(|_| LibretroError::CoreNotLoaded)?;
            func();
            Ok(())
        }
    }

    pub fn deinit(&self) -> Result<(), LibretroError> {
        unsafe {
            let func: Symbol<extern "C" fn()> = self
                .library
                .get(b"retro_deinit")
                .map_err(|_| LibretroError::CoreNotLoaded)?;
            func();
            Ok(())
        }
    }
}

// C struct representations
#[repr(C)]
struct RetroSystemInfoC {
    library_name: *const c_char,
    library_version: *const c_char,
    valid_extensions: *const c_char,
    need_fullpath: bool,
    block_extract: bool,
}

#[repr(C)]
struct RetroGameInfoC {
    path: *const c_char,
    data: *const c_void,
    size: usize,
    meta: *const c_char,
}

fn cstring_to_string(ptr: *const c_char) -> String {
    if ptr.is_null() {
        String::new()
    } else {
        unsafe {
            std::ffi::CStr::from_ptr(ptr)
                .to_string_lossy()
                .into_owned()
        }
    }
}
