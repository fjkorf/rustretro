use libloading::{Library, Symbol};
use std::ffi::{c_char, c_void};
use std::path::Path;
use thiserror::Error;

pub const RETRO_API_VERSION: u32 = 1;

// Environment callback commands - values from libretro.h
// https://github.com/libretro/libretro-common/blob/master/include/libretro.h
pub const RETRO_ENVIRONMENT_EXPERIMENTAL: u32 = 0x10000;

pub const RETRO_ENVIRONMENT_SET_ROTATION: u32 = 1;
pub const RETRO_ENVIRONMENT_GET_OVERSCAN: u32 = 2;
pub const RETRO_ENVIRONMENT_GET_CAN_DUPE: u32 = 3;
pub const RETRO_ENVIRONMENT_SET_MESSAGE: u32 = 6;
pub const RETRO_ENVIRONMENT_SHUTDOWN: u32 = 7;
pub const RETRO_ENVIRONMENT_SET_PERFORMANCE_LEVEL: u32 = 8;
pub const RETRO_ENVIRONMENT_GET_SYSTEM_DIRECTORY: u32 = 9;
pub const RETRO_ENVIRONMENT_SET_PIXEL_FORMAT: u32 = 10;
pub const RETRO_ENVIRONMENT_SET_INPUT_DESCRIPTORS: u32 = 11;
pub const RETRO_ENVIRONMENT_GET_VARIABLE: u32 = 15;
pub const RETRO_ENVIRONMENT_SET_VARIABLES: u32 = 16;
pub const RETRO_ENVIRONMENT_GET_VARIABLE_UPDATE: u32 = 17;
pub const RETRO_ENVIRONMENT_SET_SUPPORT_NO_GAME: u32 = 18;
pub const RETRO_ENVIRONMENT_GET_LOG_INTERFACE: u32 = 27;
pub const RETRO_ENVIRONMENT_GET_PERF_INTERFACE: u32 = 28;
pub const RETRO_ENVIRONMENT_GET_SAVE_DIRECTORY: u32 = 31;
pub const RETRO_ENVIRONMENT_SET_SYSTEM_AV_INFO: u32 = 32;
pub const RETRO_ENVIRONMENT_SET_SUBSYSTEM_INFO: u32 = 34;
pub const RETRO_ENVIRONMENT_SET_CONTROLLER_INFO: u32 = 35;
pub const RETRO_ENVIRONMENT_SET_GEOMETRY: u32 = 37;
pub const RETRO_ENVIRONMENT_GET_USERNAME: u32 = 38;
pub const RETRO_ENVIRONMENT_GET_LANGUAGE: u32 = 39;
pub const RETRO_ENVIRONMENT_SET_SERIALIZATION_QUIRKS: u32 = 44;
pub const RETRO_ENVIRONMENT_GET_CORE_OPTIONS_VERSION: u32 = 52;
pub const RETRO_ENVIRONMENT_SET_CORE_OPTIONS: u32 = 53;
pub const RETRO_ENVIRONMENT_SET_CORE_OPTIONS_INTL: u32 = 54;
pub const RETRO_ENVIRONMENT_SET_CORE_OPTIONS_DISPLAY: u32 = 55;
pub const RETRO_ENVIRONMENT_SET_AUDIO_BUFFER_STATUS_CALLBACK: u32 = 62;
pub const RETRO_ENVIRONMENT_SET_CORE_OPTIONS_V2: u32 = 67;
pub const RETRO_ENVIRONMENT_SET_CORE_OPTIONS_V2_INTL: u32 = 68;
// Experimental callbacks (base | 0x10000)
pub const RETRO_ENVIRONMENT_GET_VFS_INTERFACE: u32 = 45 | RETRO_ENVIRONMENT_EXPERIMENTAL; // 65581
pub const RETRO_ENVIRONMENT_GET_LED_INTERFACE: u32 = 46 | RETRO_ENVIRONMENT_EXPERIMENTAL; // 65582
pub const RETRO_ENVIRONMENT_GET_AUDIO_VIDEO_ENABLE: u32 = 47 | RETRO_ENVIRONMENT_EXPERIMENTAL; // 65583
pub const RETRO_ENVIRONMENT_SET_MEMORY_MAPS: u32 = 36 | RETRO_ENVIRONMENT_EXPERIMENTAL; // 65572

// Memory descriptor flags
pub const RETRO_MEMDESC_CONST: u64 = 1 << 0;
pub const RETRO_MEMDESC_BIGENDIAN: u64 = 1 << 1;
pub const RETRO_MEMDESC_SYSTEM_RAM: u64 = 1 << 2;
pub const RETRO_MEMDESC_SAVE_RAM: u64 = 1 << 3;
pub const RETRO_MEMDESC_VIDEO_RAM: u64 = 1 << 4;

// Pixel format constants (retro_pixel_format enum values)
pub const RETRO_PIXEL_FORMAT_0RGB1555: u32 = 0; // legacy default
pub const RETRO_PIXEL_FORMAT_XRGB8888: u32 = 1;
pub const RETRO_PIXEL_FORMAT_RGB565: u32 = 2;

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

// C-compatible layout matching libretro.h retro_system_av_info
#[repr(C)]
#[derive(Copy, Clone, Debug, Default)]
pub struct RetroSystemAVInfoC {
    pub base_width: u32,
    pub base_height: u32,
    pub max_width: u32,
    pub max_height: u32,
    pub aspect_ratio: f32,
    pub fps: f64,
    pub sample_rate: f64,
}

impl RetroSystemAVInfoC {
    pub fn to_rust(&self) -> RetroSystemAVInfo {
        let aspect = if self.aspect_ratio <= 0.0 {
            self.base_width as f32 / self.base_height as f32
        } else {
            self.aspect_ratio
        };
        RetroSystemAVInfo {
            geometry: RetroGameGeometry {
                base_width: self.base_width,
                base_height: self.base_height,
                max_width: self.max_width,
                max_height: self.max_height,
                aspect_ratio: aspect,
            },
            timing: RetroSystemTiming {
                fps: self.fps,
                sample_rate: self.sample_rate,
            },
        }
    }
}

pub type RetroEnvironmentFn = extern "C" fn(cmd: u32, data: *mut c_void) -> bool;
pub type RetroVideoRefreshFn = extern "C" fn(data: *const c_void, width: u32, height: u32, pitch: usize);
pub type RetroAudioSampleFn = extern "C" fn(left: i16, right: i16);
pub type RetroAudioSampleBatchFn = extern "C" fn(data: *const i16, frames: usize) -> usize;
pub type RetroInputPollFn = extern "C" fn();
pub type RetroInputStateFn = extern "C" fn(port: u32, device: u32, index: u32, id: u32) -> i16;
pub type RetroCoreLogFn = unsafe extern "C" fn(level: u32, msg: *const std::ffi::c_char);

#[repr(C)]
pub struct RetroLogCallback {
    pub log: *const c_void, // RetroCoreLogFn cast to *const c_void
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct RetroMessage {
    pub msg: *const std::ffi::c_char,
    pub frames: u32,
}

#[repr(C)]
#[derive(Clone)]
pub struct RetroMemoryDescriptor {
    pub flags: u64,
    pub ptr: *mut c_void,
    pub offset: usize,
    pub start: usize,
    pub select: usize,
    pub disconnect: usize,
    pub len: usize,
    pub addrspace: *const c_char,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct RetroMemoryMap {
    pub descriptors: *const RetroMemoryDescriptor,
    pub num_descriptors: u32,
}

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

            let path_ptr = c_path.as_ptr();
            let _leaked_path = Box::leak(Box::new(c_path));

            let rom_data = game.data.clone();
            let rom_size = rom_data.len();
            let data_ptr = if !rom_data.is_empty() {
                Box::leak(Box::new(rom_data)).as_ptr() as *const c_void
            } else {
                std::ptr::null()
            };

            let game_info_ptr = Box::into_raw(Box::new(RetroGameInfoC {
                path: path_ptr,
                data: data_ptr,
                size: rom_size,
                meta: std::ptr::null(),
            }));

            let result = if func(game_info_ptr) {
                eprintln!("✅ load_game() returned true");
                Ok(())
            } else {
                eprintln!("❌ load_game() returned false");
                Err(LibretroError::GameLoadFailed)
            };

            // Don't free — core may keep a pointer into this memory
            let _ = game_info_ptr;

            result
        }
    }

    pub fn get_av_info(&self) -> Result<RetroSystemAVInfo, LibretroError> {
        unsafe {
            let func: Symbol<extern "C" fn(*mut RetroSystemAVInfoC)> = self
                .library
                .get(b"retro_get_system_av_info")
                .map_err(|_| LibretroError::CoreNotLoaded)?;
            let mut info = RetroSystemAVInfoC::default();
            func(&mut info);
            Ok(info.to_rust())
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

    // ========================================================================
    // Debug APIs (fbalpha2012)
    // ========================================================================

    pub fn get_m68k_register(&self, reg: SekRegister) -> Result<u32, LibretroError> {
        unsafe {
            // Try both symbol name variations
            let symbol_name = b"_Z17SekDbgGetRegister11SekRegister";
            match self.library.get::<Symbol<SekDbgGetRegisterFn>>(symbol_name) {
                Ok(func) => Ok(func(reg)),
                Err(e) => {
                    eprintln!("[LIBLOAD] SekDbgGetRegister ({:?}) failed: {}", String::from_utf8_lossy(symbol_name), e);
                    Err(LibretroError::CoreNotLoaded)
                }
            }
        }
    }

    pub fn set_m68k_register(&self, reg: SekRegister, value: u32) -> Result<bool, LibretroError> {
        unsafe {
            let func: Symbol<SekDbgSetRegisterFn> = self
                .library
                .get(b"_Z17SekDbgSetRegister11SekRegisterj")
                .map_err(|_| LibretroError::CoreNotLoaded)?;
            Ok(func(reg, value))
        }
    }

    pub fn get_m68k_cpu_type(&self) -> Result<i32, LibretroError> {
        unsafe {
            let func: Symbol<SekDbgGetCPUTypeFn> = self
                .library
                .get(b"_Z16SekDbgGetCPUTypev")
                .map_err(|_| LibretroError::CoreNotLoaded)?;
            Ok(func())
        }
    }

    pub fn get_m68k_pending_irq(&self) -> Result<i32, LibretroError> {
        unsafe {
            let func: Symbol<SekDbgGetPendingIRQFn> = self
                .library
                .get(b"_Z19SekDbgGetPendingIRQv")
                .map_err(|_| LibretroError::CoreNotLoaded)?;
            Ok(func())
        }
    }

    /// Read `count` bytes from M68K address space starting at `addr` using
    /// SekFetchByte (instruction fetch — no I/O side effects).
    /// Returns an empty Vec if the symbol is unavailable (non-fbalpha2012 core).
    pub fn read_m68k_code(&self, addr: u32, count: usize) -> Vec<u8> {
        unsafe {
            match self.library.get::<Symbol<SekFetchByteFn>>(b"_Z12SekFetchBytej") {
                Ok(fetch) => (0..count as u32)
                    .map(|i| fetch(addr.wrapping_add(i)))
                    .collect(),
                Err(_) => Vec::new(),
            }
        }
    }

    pub fn get_z80_pc(&self, cpu: i32) -> Result<i32, LibretroError> {
        unsafe {
            let func: Symbol<ZetGetPCFn> = self
                .library
                .get(b"_Z8ZetGetPCi")
                .map_err(|_| LibretroError::CoreNotLoaded)?;
            Ok(func(cpu))
        }
    }

    pub fn get_z80_bc(&self, cpu: i32) -> Result<i32, LibretroError> {
        unsafe {
            let func: Symbol<ZetBcFn> = self
                .library
                .get(b"_Z5ZetBci")
                .map_err(|_| LibretroError::CoreNotLoaded)?;
            Ok(func(cpu))
        }
    }

    pub fn get_z80_de(&self, cpu: i32) -> Result<i32, LibretroError> {
        unsafe {
            let func: Symbol<ZetDeFn> = self
                .library
                .get(b"_Z5ZetDei")
                .map_err(|_| LibretroError::CoreNotLoaded)?;
            Ok(func(cpu))
        }
    }

    pub fn get_z80_hl(&self, cpu: i32) -> Result<i32, LibretroError> {
        unsafe {
            let func: Symbol<ZetHLFn> = self
                .library
                .get(b"_Z5ZetHLi")
                .map_err(|_| LibretroError::CoreNotLoaded)?;
            Ok(func(cpu))
        }
    }

    pub fn get_z80_active(&self) -> Result<i32, LibretroError> {
        unsafe {
            let func: Symbol<ZetGetActiveFn> = self
                .library
                .get(b"_Z12ZetGetActivev")
                .map_err(|_| LibretroError::CoreNotLoaded)?;
            Ok(func())
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

// ============================================================================
// M68000 CPU Debug API (from fbalpha2012)
// ============================================================================

#[repr(C)]
pub enum SekRegister {
    D0, D1, D2, D3, D4, D5, D6, D7,
    A0, A1, A2, A3, A4, A5, A6, A7,
    PC,
    SR,
    SP,
    USP,
    ISP,
    MSP,
    VBR,
    SFC,
    DFC,
    CACR,
    CAAR,
}

pub type SekDbgGetRegisterFn = extern "C" fn(SekRegister) -> u32;
pub type SekDbgSetRegisterFn = extern "C" fn(SekRegister, u32) -> bool;
pub type SekDbgGetCPUTypeFn = extern "C" fn() -> i32;
pub type SekDbgGetPendingIRQFn = extern "C" fn() -> i32;
pub type SekFetchByteFn = extern "C" fn(u32) -> u8;

// ============================================================================
// Z80 CPU Debug API (from fbalpha2012)
// ============================================================================

pub type ZetGetPCFn = extern "C" fn(i32) -> i32;
pub type ZetBcFn = extern "C" fn(i32) -> i32;
pub type ZetDeFn = extern "C" fn(i32) -> i32;
pub type ZetHLFn = extern "C" fn(i32) -> i32;
pub type ZetGetActiveFn = extern "C" fn() -> i32;
