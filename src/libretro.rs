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

// retro_get_memory_data / retro_get_memory_size id values (libretro.h
// RETRO_MEMORY_*). Used by the SET_MEMORY_MAPS fallback: many cores
// (fbalpha2012, Genesis Plus GX, FBNeo) never publish a memory map but DO
// implement this simpler pointer+size interface for work RAM / VRAM.
pub const RETRO_MEMORY_SAVE_RAM: u32 = 0;
pub const RETRO_MEMORY_RTC: u32 = 1;
pub const RETRO_MEMORY_SYSTEM_RAM: u32 = 2;
pub const RETRO_MEMORY_VIDEO_RAM: u32 = 3;

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
#[repr(C)]
pub struct RetroLogCallback {
    // retro_log_printf_t (C-variadic: fn(level, fmt, ...)) cast to *const
    // c_void. The actual function is rr_core_log in src/log_shim.c.
    pub log: *const c_void,
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

    // ========================================================================
    // Save states (retro_serialize / retro_unserialize)
    // ========================================================================

    /// `retro_serialize_size()` — size in bytes of a serialized state, or 0 if
    /// the symbol is missing or the core reports no state.
    pub fn serialize_size(&self) -> usize {
        unsafe {
            match self
                .library
                .get::<Symbol<RetroSerializeSizeFn>>(b"retro_serialize_size")
            {
                Ok(func) => func(),
                Err(_) => 0,
            }
        }
    }

    /// Serialize the core's full machine state into an owned buffer.
    /// Returns `None` when the core exposes no serialization (size 0 / missing
    /// symbol) or `retro_serialize` reports failure.
    pub fn serialize(&self) -> Option<Vec<u8>> {
        let size = self.serialize_size();
        if size == 0 {
            return None;
        }
        unsafe {
            let func = self
                .library
                .get::<Symbol<RetroSerializeFn>>(b"retro_serialize")
                .ok()?;
            let mut buf = vec![0u8; size];
            if func(buf.as_mut_ptr() as *mut c_void, size) {
                Some(buf)
            } else {
                None
            }
        }
    }

    /// Restore a previously serialized state. Returns false when the symbol is
    /// missing, `data` is empty, or the core rejects the buffer (wrong size /
    /// version / game).
    pub fn unserialize(&self, data: &[u8]) -> bool {
        if data.is_empty() {
            return false;
        }
        unsafe {
            match self
                .library
                .get::<Symbol<RetroUnserializeFn>>(b"retro_unserialize")
            {
                Ok(func) => func(data.as_ptr() as *const c_void, data.len()),
                Err(_) => false,
            }
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
            let symbol_name = b"_Z17SekDbgGetRegister11SekRegister";
            match self.library.get::<Symbol<SekDbgGetRegisterFn>>(symbol_name) {
                Ok(func) => Ok(func(reg)),
                Err(e) => {
                    // This probe runs every frame on every core; a non-FBA core
                    // will never grow the symbol, so warn exactly once.
                    static WARN_ONCE: std::sync::Once = std::sync::Once::new();
                    WARN_ONCE.call_once(|| {
                        eprintln!(
                            "[LIBLOAD] SekDbgGetRegister ({:?}) failed: {} \
                             (core has no 68k debug API; further probes are silent)",
                            String::from_utf8_lossy(symbol_name),
                            e
                        );
                    });
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

    /// Bulk-read `len` bytes of the live M68K bus via the core's exported
    /// SekReadByte/SekReadLong (fbalpha2012). Bytes come back in guest
    /// (big-endian) order: `out[i]` is the byte the CPU sees at `addr + i`.
    ///
    /// Returns `None` when the core does not export the full Sek read API —
    /// including the SekGetActive/SekOpen/SekClose guard trio, because a read
    /// with no open CPU context dereferences a null memory map inside the core.
    /// `DebugCPU_SekInitted` (when exported) additionally rejects cores whose
    /// 68k was never initialised (e.g. a Z80-only game on fbalpha2012).
    ///
    /// Symbols are resolved once per call, not once per read: a bus-window
    /// refresh makes ~50k reads per frame and per-read dlsym would dominate.
    pub fn sek_read_block(&self, addr: u32, len: usize) -> Option<Vec<u8>> {
        if len == 0 {
            return Some(Vec::new());
        }
        unsafe {
            if let Ok(initted) = self.library.get::<Symbol<*const u8>>(b"DebugCPU_SekInitted") {
                let addr: *const u8 = **initted;
                if !addr.is_null() && *addr == 0 {
                    return None;
                }
            }
            let read_byte = self
                .library
                .get::<Symbol<SekReadByteFn>>(b"_Z11SekReadBytej")
                .ok()?;
            let read_long = self
                .library
                .get::<Symbol<SekReadLongFn>>(b"_Z11SekReadLongj")
                .ok()?;
            let get_active = self
                .library
                .get::<Symbol<SekGetActiveFn>>(b"_Z12SekGetActivev")
                .ok()?;
            let open = self.library.get::<Symbol<SekOpenFn>>(b"_Z7SekOpeni").ok()?;
            let close = self.library.get::<Symbol<SekCloseFn>>(b"_Z8SekClosev").ok()?;

            let opened_here = get_active() < 0;
            if opened_here {
                open(0);
            }
            let (head, longs, tail) = plan_block_reads(addr, len);
            let mut out = Vec::with_capacity(len);
            for a in head {
                out.push(read_byte(a as u32));
            }
            let mut a = longs.start;
            while a < longs.end {
                out.extend_from_slice(&read_long(a as u32).to_be_bytes());
                a += 4;
            }
            for a in tail {
                out.push(read_byte(a as u32));
            }
            if opened_here {
                close();
            }
            Some(out)
        }
    }

    /// Write `bytes` to the live 68k bus starting at `addr` via the core's
    /// exported SekWriteByte (fbalpha2012). Byte-wise (no long packing) so it is
    /// endianness-symmetric with `sek_read_block`: byte `i` goes to bus `addr+i`,
    /// the same position `sek_read_block` reads it back into. Returns false when
    /// the core does not export the guarded Sek API (same probe/guard as the read
    /// path — a write with no open CPU context would deref a null memory map).
    pub fn sek_write_block(&self, addr: u32, bytes: &[u8]) -> bool {
        if bytes.is_empty() {
            return true;
        }
        unsafe {
            if let Ok(initted) = self.library.get::<Symbol<*const u8>>(b"DebugCPU_SekInitted") {
                let p: *const u8 = **initted;
                if !p.is_null() && *p == 0 {
                    return false;
                }
            }
            let write_byte = match self.library.get::<Symbol<SekWriteByteFn>>(b"_Z12SekWriteBytejh") {
                Ok(f) => f,
                Err(_) => return false,
            };
            let get_active = match self.library.get::<Symbol<SekGetActiveFn>>(b"_Z12SekGetActivev") {
                Ok(f) => f,
                Err(_) => return false,
            };
            let open = match self.library.get::<Symbol<SekOpenFn>>(b"_Z7SekOpeni") {
                Ok(f) => f,
                Err(_) => return false,
            };
            let close = match self.library.get::<Symbol<SekCloseFn>>(b"_Z8SekClosev") {
                Ok(f) => f,
                Err(_) => return false,
            };
            let opened_here = get_active() < 0;
            if opened_here {
                open(0);
            }
            for (i, b) in bytes.iter().enumerate() {
                write_byte(addr.wrapping_add(i as u32), *b);
            }
            if opened_here {
                close();
            }
            true
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

    /// Resolve `retro_get_memory_data(id)` — returns a host pointer to the
    /// requested memory block (e.g. system work RAM) owned by the core, or null
    /// if the symbol is missing or the core has no such block.
    ///
    /// The returned pointer is NOT dereferenced here; callers store it (as
    /// `usize`) in a `MemoryRegion` and read through the guarded
    /// `safe_host_ptr` path.
    pub fn get_memory_data(&self, id: u32) -> *mut std::ffi::c_void {
        unsafe {
            match self.library.get::<Symbol<RetroGetMemoryDataFn>>(b"retro_get_memory_data") {
                Ok(func) => func(id as std::ffi::c_uint),
                Err(_) => std::ptr::null_mut(),
            }
        }
    }

    /// Resolve `retro_get_memory_size(id)` — returns the size in bytes of the
    /// requested memory block, or 0 if the symbol is missing / the block is
    /// unavailable.
    pub fn get_memory_size(&self, id: u32) -> usize {
        unsafe {
            match self.library.get::<Symbol<RetroGetMemorySizeFn>>(b"retro_get_memory_size") {
                Ok(func) => func(id as std::ffi::c_uint),
                Err(_) => 0,
            }
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

// Sek bus access (fbalpha2012 exports its 68k memory API; INT32 returns ignored)
pub type SekReadByteFn = extern "C" fn(u32) -> u8;
pub type SekReadWordFn = extern "C" fn(u32) -> u16;
pub type SekReadLongFn = extern "C" fn(u32) -> u32;
pub type SekWriteByteFn = extern "C" fn(u32, u8);
pub type SekWriteWordFn = extern "C" fn(u32, u16);
pub type SekWriteLongFn = extern "C" fn(u32, u32);
pub type SekGetActiveFn = extern "C" fn() -> i32;
pub type SekOpenFn = extern "C" fn(i32) -> i32;
pub type SekCloseFn = extern "C" fn() -> i32;

/// Split an `(addr, len)` bus span into unaligned head bytes, aligned 4-byte
/// longs, and tail bytes (all as `u64` address ranges, so `addr + len` can't
/// wrap). Pure so the alignment arithmetic is unit-testable without a core.
pub fn plan_block_reads(
    addr: u32,
    len: usize,
) -> (
    std::ops::Range<u64>,
    std::ops::Range<u64>,
    std::ops::Range<u64>,
) {
    let start = addr as u64;
    let end = start + len as u64;
    // Head runs to the next 4-byte boundary, but never past the span's end.
    let head_end = end.min((start + 3) & !3);
    // Longs cover whole 4-byte words between the head and the last boundary.
    let long_end = head_end.max(end & !3);
    (start..head_end, head_end..long_end, long_end..end)
}

// retro_get_memory_data(unsigned) -> void* ; retro_get_memory_size(unsigned) -> size_t
pub type RetroGetMemoryDataFn = extern "C" fn(std::ffi::c_uint) -> *mut c_void;
pub type RetroGetMemorySizeFn = extern "C" fn(std::ffi::c_uint) -> usize;

// Save states: retro_serialize_size() -> size_t;
// retro_serialize(void*, size_t) -> bool; retro_unserialize(const void*, size_t) -> bool
pub type RetroSerializeSizeFn = extern "C" fn() -> usize;
pub type RetroSerializeFn = extern "C" fn(*mut c_void, usize) -> bool;
pub type RetroUnserializeFn = extern "C" fn(*const c_void, usize) -> bool;

// ============================================================================
// Z80 CPU Debug API (from fbalpha2012)
// ============================================================================

pub type ZetGetPCFn = extern "C" fn(i32) -> i32;
pub type ZetBcFn = extern "C" fn(i32) -> i32;
pub type ZetDeFn = extern "C" fn(i32) -> i32;
pub type ZetHLFn = extern "C" fn(i32) -> i32;
pub type ZetGetActiveFn = extern "C" fn() -> i32;

#[cfg(test)]
mod tests {
    use super::plan_block_reads;

    /// Reassemble a plan into the flat list of addresses each byte comes from,
    /// tagging long-reads so tests can also assert the read widths.
    fn covered(addr: u32, len: usize) -> Vec<u64> {
        let (head, longs, tail) = plan_block_reads(addr, len);
        assert_eq!(head.start, addr as u64);
        assert_eq!(head.end, longs.start);
        assert_eq!(longs.end, tail.start);
        assert_eq!(tail.end, addr as u64 + len as u64);
        assert_eq!(longs.start % 4, if longs.is_empty() { longs.start % 4 } else { 0 });
        assert_eq!((longs.end - longs.start) % 4, 0);
        head.chain(longs).chain(tail).collect()
    }

    #[test]
    fn plan_covers_every_byte_exactly_once() {
        for addr in 0u32..8 {
            for len in 0usize..20 {
                let bytes = covered(addr, len);
                let expect: Vec<u64> = (addr as u64..addr as u64 + len as u64).collect();
                assert_eq!(bytes, expect, "addr={addr} len={len}");
            }
        }
    }

    #[test]
    fn plan_small_unaligned_span_is_all_head() {
        let (head, longs, tail) = plan_block_reads(1, 2);
        assert_eq!((head.start, head.end), (1, 3));
        assert!(longs.is_empty());
        assert!(tail.is_empty());
    }

    #[test]
    fn plan_aligned_span_is_all_longs() {
        let (head, longs, tail) = plan_block_reads(0x500000, 0x2000);
        assert!(head.is_empty());
        assert_eq!((longs.start, longs.end), (0x500000, 0x502000));
        assert!(tail.is_empty());
    }

    #[test]
    fn plan_no_overflow_at_top_of_address_space() {
        let (head, longs, tail) = plan_block_reads(u32::MAX - 3, 4);
        let total = (head.end - head.start) + (longs.end - longs.start) + (tail.end - tail.start);
        assert_eq!(total, 4);
    }
}
