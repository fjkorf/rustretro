use crate::debug::{Bookmark, SharedDebugState};
use crate::libretro::*;
use anyhow::{anyhow, Result};
use std::ffi::{CString, c_uint, c_void};
use std::path::PathBuf;
use std::sync::{Arc, atomic::{AtomicPtr, Ordering}};

// Global static for callback context access during libretro callbacks
static CALLBACK_CONTEXT: AtomicPtr<CallbackContext> = AtomicPtr::new(std::ptr::null_mut());

pub struct Frontend {
    core: RetroCore,
    pub av_info: Option<RetroSystemAVInfo>,
    callback_context: Box<CallbackContext>,
    _game_path_cstring: Option<CString>,
    pub frame_count: u64,
    debug_state: SharedDebugState,
    sidecar_path: Option<std::path::PathBuf>,
    /// Ensures the SET_MEMORY_MAPS fallback synthesis runs at most once.
    did_memory_fallback: bool,
}

impl Frontend {
    pub fn new(
        core_path: &str,
        rom_path: &str,
        save_dir: PathBuf,
        system_dir: PathBuf,
        debug_state: SharedDebugState,
    ) -> Result<Self> {
        let core = RetroCore::load(core_path)
            .map_err(|e| anyhow!("Failed to load core: {}", e))?;

        let system_info = core
            .get_system_info()
            .map_err(|e| anyhow!("Failed to get system info: {}", e))?;

        eprintln!("Core: {} v{}", system_info.library_name, system_info.library_version);
        eprintln!("Valid extensions: {}", system_info.valid_extensions);

        let callback_context = Box::new(CallbackContext::new(save_dir.clone(), system_dir, Arc::clone(&debug_state)));

        // Derive sidecar path: <save_dir>/<rom_stem>.regions.json
        let sidecar_path = std::path::Path::new(rom_path)
            .file_stem()
            .map(|stem| save_dir.join(format!("{}.regions.json", stem.to_string_lossy())));

        let mut frontend = Frontend {
            core,
            av_info: None,
            callback_context,
            _game_path_cstring: None,
            frame_count: 0,
            debug_state: Arc::clone(&debug_state),
            sidecar_path: sidecar_path.clone(),
            did_memory_fallback: false,
        };

        // Store sidecar path in debug state and try to load existing data
        if let Ok(mut ds) = debug_state.lock() {
            ds.sidecar_path = sidecar_path.clone();
        }
        if let Some(ref path) = sidecar_path {
            load_regions_sidecar(path, &debug_state);
        }

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

        // SET_MEMORY_MAPS env callbacks (if any) have already fired during
        // load_game, so debug_state.memory_regions is now populated for cores
        // that publish a map. For cores that DON'T (fbalpha2012, Genesis Plus
        // GX, FBNeo), synthesize a region from retro_get_memory_data/size.
        frontend.apply_memory_map_fallback();

        if let Ok(av_info) = frontend.core.get_av_info() {
            let w = av_info.geometry.base_width;
            let h = av_info.geometry.base_height;
            eprintln!(
                "AV info: {}x{} @ {:.2} FPS, {:.0} Hz audio",
                w, h, av_info.timing.fps, av_info.timing.sample_rate
            );
            frontend.av_info = Some(av_info);
        }

        Ok(frontend)
    }

    /// Synthesize memory regions for cores that never call SET_MEMORY_MAPS but
    /// do implement retro_get_memory_data/size (fbalpha2012/CPS2, Genesis Plus
    /// GX, FBNeo). Without this, the whole memory-perception layer (read_memory,
    /// read_region, search_memory, Lua memory.read_*) has zero regions to
    /// address.
    ///
    /// Runs at most once (guarded by `did_memory_fallback`). Purely additive: if
    /// the core DID publish a map, `memory_regions` is non-empty and we return
    /// without touching it.
    fn apply_memory_map_fallback(&mut self) {
        if self.did_memory_fallback {
            return;
        }

        // If the core already published a real memory map, we're done — stop
        // retrying. Otherwise we may need to retry on later frames: some cores
        // (e.g. Genesis Plus GX) don't return a valid get_memory_data pointer
        // until after the first retro_run, so a null result here must NOT
        // permanently disable the fallback.
        {
            let ds = match self.debug_state.lock() {
                Ok(ds) => ds,
                Err(_) => return,
            };
            if !ds.memory_regions.is_empty() {
                self.did_memory_fallback = true;
                return;
            }
        }

        let mut synthesized: Vec<crate::debug::MemoryRegion> = Vec::new();

        // System work RAM at guest base 0 — the common case that unlocks
        // work-RAM reads (e.g. Genesis MK II, CPS2 MvC).
        let sysram_ptr = self.core.get_memory_data(RETRO_MEMORY_SYSTEM_RAM);
        let sysram_size = self.core.get_memory_size(RETRO_MEMORY_SYSTEM_RAM);
        if !sysram_ptr.is_null() && sysram_size > 0 {
            synthesized.push(crate::debug::MemoryRegion::synth_region(
                "System RAM (fallback)",
                0,
                sysram_size,
                sysram_ptr as usize,
                RETRO_MEMDESC_SYSTEM_RAM,
            ));
        }

        // Video RAM if the core exposes it. Placed at a high, non-overlapping
        // guest base so its addresses are distinct from System RAM; VRAM is
        // addressed by region name anyway (vram_to_rom / read_region), so the
        // exact base only needs to avoid colliding with System RAM (base 0).
        const VRAM_FALLBACK_BASE: usize = 0x1000_0000;
        let vram_ptr = self.core.get_memory_data(RETRO_MEMORY_VIDEO_RAM);
        let vram_size = self.core.get_memory_size(RETRO_MEMORY_VIDEO_RAM);
        if !vram_ptr.is_null() && vram_size > 0 {
            synthesized.push(crate::debug::MemoryRegion::synth_region(
                "Video RAM (fallback)",
                VRAM_FALLBACK_BASE,
                vram_size,
                vram_ptr as usize,
                RETRO_MEMDESC_VIDEO_RAM,
            ));
        }

        if synthesized.is_empty() {
            return;
        }

        if let Ok(mut ds) = self.debug_state.lock() {
            // Re-check under the lock in case a map arrived meanwhile.
            if !ds.memory_regions.is_empty() {
                return;
            }
            for r in &synthesized {
                ds.log(format!(
                    "[mem] no core memory map; synthesized {} region: {} bytes @ host ptr 0x{:x}",
                    r.region_type(),
                    r.size,
                    r.ptr,
                ));
            }
            ds.memory_regions = synthesized;
        }
        // Only stop retrying once we've actually synthesized something (or the
        // core published a map — handled above). A null get_memory_data at
        // load time leaves did_memory_fallback false so we retry next frame.
        self.did_memory_fallback = true;
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

    /// Width of the emulated video frame (may be 0 before first frame).
    pub fn video_width(&self) -> u32 {
        self.callback_context.width
            .max(self.av_info.as_ref().map_or(0, |a| a.geometry.base_width))
    }

    /// Height of the emulated video frame.
    pub fn video_height(&self) -> u32 {
        self.callback_context.height
            .max(self.av_info.as_ref().map_or(0, |a| a.geometry.base_height))
    }

    /// Target FPS reported by the core.
    pub fn fps(&self) -> f64 {
        self.av_info.as_ref().map_or(60.0, |a| a.timing.fps)
    }

    /// Target audio sample rate.
    pub fn sample_rate(&self) -> f64 {
        self.av_info.as_ref().map_or(44100.0, |a| a.timing.sample_rate)
    }

    /// Push controller state into the callback context before calling run_frame().
    pub fn set_input(&mut self, state: [bool; 12]) {
        self.callback_context.input_state = state;
    }

    /// Capture M68K and Z80 CPU state from the core (fbalpha2012-specific).
    fn capture_cpu_state(&self) {
        if let Ok(mut ds) = self.debug_state.try_lock() {
            let mut any_success = false;

            // Save previous register values for delta highlighting before overwriting
            ds.prev_m68k_d_regs = ds.m68k_d_regs;
            ds.prev_m68k_a_regs = ds.m68k_a_regs;
            ds.prev_m68k_pc = ds.m68k_pc;

            // Try to read M68K registers (D0-D7)
            for i in 0..8 {
                let reg = match i {
                    0 => SekRegister::D0, 1 => SekRegister::D1, 2 => SekRegister::D2, 3 => SekRegister::D3,
                    4 => SekRegister::D4, 5 => SekRegister::D5, 6 => SekRegister::D6, 7 => SekRegister::D7,
                    _ => continue,
                };
                match self.core.get_m68k_register(reg) {
                    Ok(val) => {
                        ds.m68k_d_regs[i as usize] = val;
                        any_success = true;
                    }
                    Err(e) => {
                        if i == 0 && self.frame_count % 300 == 0 {
                            eprintln!("[CPU] M68K D{} read failed: {}", i, e);
                        }
                    }
                }
            }
            
            // A0-A7
            for i in 0..8 {
                let reg = match i {
                    0 => SekRegister::A0, 1 => SekRegister::A1, 2 => SekRegister::A2, 3 => SekRegister::A3,
                    4 => SekRegister::A4, 5 => SekRegister::A5, 6 => SekRegister::A6, 7 => SekRegister::A7,
                    _ => continue,
                };
                match self.core.get_m68k_register(reg) {
                    Ok(val) => {
                        ds.m68k_a_regs[i as usize] = val;
                        any_success = true;
                    }
                    Err(e) => {
                        if i == 0 && self.frame_count % 300 == 0 {
                            eprintln!("[CPU] M68K A{} read failed: {}", i, e);
                        }
                    }
                }
            }
            
            // PC and SR
            match self.core.get_m68k_register(SekRegister::PC) {
                Ok(pc) => {
                    ds.m68k_pc = pc;
                    *ds.pc_heatmap.entry(pc).or_insert(0) += 1;
                    any_success = true;
                }
                Err(e) => {
                    if self.frame_count % 300 == 0 {
                        eprintln!("[CPU] M68K PC read failed: {}", e);
                    }
                }
            }
            match self.core.get_m68k_register(SekRegister::SR) {
                Ok(sr) => {
                    ds.m68k_sr = sr;
                    any_success = true;
                }
                Err(e) => {
                    if self.frame_count % 300 == 0 {
                        eprintln!("[CPU] M68K SR read failed: {}", e);
                    }
                }
            }

            // Try to read Z80 registers (need to be careful about which CPU)
            match self.core.get_z80_pc(0) {
                Ok(pc) => {
                    ds.z80_pc = (pc & 0xFFFF) as u16;
                    any_success = true;
                }
                Err(e) => {
                    if self.frame_count % 300 == 0 {
                        eprintln!("[CPU] Z80 PC read failed: {}", e);
                    }
                }
            }
            match self.core.get_z80_bc(0) {
                Ok(bc) => {
                    ds.z80_bc = (bc & 0xFFFF) as u16;
                    any_success = true;
                }
                Err(_) => {}
            }
            match self.core.get_z80_de(0) {
                Ok(de) => {
                    ds.z80_de = (de & 0xFFFF) as u16;
                    any_success = true;
                }
                Err(_) => {}
            }
            match self.core.get_z80_hl(0) {
                Ok(hl) => {
                    ds.z80_hl = (hl & 0xFFFF) as u16;
                    any_success = true;
                }
                Err(_) => {}
            }

            // Populate VDP registers when a source becomes available.
            // (Currently a no-op: Genesis VDP regs are write-only and not exposed
            // by the loaded cores — see debug/vdp_source.rs for the dead-end and routes.)
            if let Some(vdp) = crate::debug::vdp_source::read_vdp_regs(&ds.memory_regions) {
                ds.vdp_regs = vdp;
            }

            // Fetch code bytes at PC for disassembly panel (256 bytes via SekFetchByte)
            if ds.m68k_pc > 0 {
                let code = self.core.read_m68k_code(ds.m68k_pc, 256);
                if !code.is_empty() {
                    ds.m68k_code_bytes = code;
                    ds.m68k_code_start = ds.m68k_pc;
                }
            }

            // Check breakpoints and run-to-addr
            let pc = ds.m68k_pc;
            if !ds.paused {
                if let Some(target) = ds.run_to_addr {
                    if pc == target {
                        ds.paused = true;
                        ds.run_to_addr = None;
                        ds.log(format!("⏸ Run-to reached ${:06X}", pc));
                    }
                }
                if !ds.paused && ds.breakpoints.contains(&pc) {
                    ds.paused = true;
                    ds.hit_breakpoint = Some(pc);
                    ds.log(format!("🔴 Breakpoint hit at ${:06X}", pc));
                }
            }

            // Update watches: read current values and apply freezes.
            // Collect ops first so we don't borrow ds.watches while calling
            // ds.read_addr / ds.write_addr (which borrow ds immutably).
            let mut freeze_writes: Vec<(usize, usize, u32)> = Vec::new();
            let mut change_events: Vec<crate::debug::ChangeEvent> = Vec::new();
            {
                let watch_params: Vec<(usize, usize, bool, Option<u32>)> = ds.watches.iter()
                    .map(|w| (w.addr, w.format.byte_len(), w.frozen, w.frozen_value))
                    .collect();
                let mut updates: Vec<(Option<u32>, Option<u32>)> = Vec::with_capacity(watch_params.len());
                for (addr, len, frozen, frozen_value) in &watch_params {
                    let current = ds.read_addr(*addr, *len);
                    let mut new_frozen_value = *frozen_value;
                    if *frozen {
                        if new_frozen_value.is_none() {
                            new_frozen_value = current;
                        }
                        if let Some(fv) = new_frozen_value {
                            freeze_writes.push((*addr, *len, fv));
                        }
                    } else {
                        new_frozen_value = None;
                    }
                    updates.push((current, new_frozen_value));
                }
                // Detect frame-granular value changes for tracked watches. We
                // collect events here (while iterating ds.watches mutably) and
                // push them after the loop, since push_change needs &mut self.
                // PC for this frame was already captured into ds.m68k_pc above.
                let frame = ds.frame_count;
                let pc = ds.m68k_pc;
                for (w, (current, new_frozen_value)) in ds.watches.iter_mut().zip(updates) {
                    if w.track_changes {
                        if let Some(cur) = current {
                            if crate::debug::detect_change(w.prev_value, cur) {
                                let old = w.prev_value.unwrap_or(cur);
                                change_events.push(crate::debug::ChangeEvent {
                                    frame,
                                    addr: w.addr,
                                    old,
                                    new: cur,
                                    pc,
                                });
                            }
                            w.prev_value = current;
                        }
                    } else {
                        // Reset edge state so re-enabling tracking starts fresh.
                        w.prev_value = None;
                    }
                    w.current = current;
                    w.frozen_value = new_frozen_value;
                }
            }
            for ev in change_events {
                ds.push_change(ev);
            }
            for (addr, len, value) in freeze_writes {
                ds.write_addr(addr, len, value);
            }

            if self.frame_count % 300 == 0 && any_success {
                eprintln!("[CPU] ✓ CPU state captured (M68K PC=${:06X})", ds.m68k_pc);
            }
        } else if self.frame_count % 300 == 0 {
            eprintln!("[CPU] Failed to acquire debug_state lock");
        }
    }

    /// If the UI requested a bookmark, capture one now and clear the flag.
    fn maybe_capture_bookmark(&self) {
        let needs_bookmark = self.debug_state.try_lock()
            .map(|ds| ds.create_bookmark)
            .unwrap_or(false);

        if !needs_bookmark { return; }

        if let Ok(mut ds) = self.debug_state.try_lock() {
            ds.create_bookmark = false;
            let frame = ds.frame_count;
            let pc    = ds.m68k_pc;
            let d     = ds.m68k_d_regs;
            let a     = ds.m68k_a_regs;
            let thumb = downsample_thumbnail(&ds.fb_rgba, ds.fb_width, ds.fb_height, 64, 48);
            let label = format!("Frame {}", frame);
            ds.bookmarks.push(Bookmark { label, frame, m68k_pc: pc, m68k_d_regs: d, m68k_a_regs: a, thumbnail: thumb, notes: String::new() });
            ds.log(format!("📌 Bookmark created at frame {} PC=${:06X}", frame, pc));
        }
    }

    /// Run exactly one emulation frame. Returns true if a new video frame was produced.
    pub fn run_frame(&mut self) -> Result<bool> {
        // Retry the memory-map fallback until it lands — some cores (Genesis
        // Plus GX) only expose get_memory_data after the first retro_run. Cheap
        // no-op once it has succeeded or a real map arrived.
        self.apply_memory_map_fallback();

        // --- Check pause / triggers ---
        let paused = {
            let mut ds = self.debug_state.lock().unwrap();
            ds.push_input(self.callback_context.input_state, self.frame_count);
            ds.frame_count = self.frame_count;

            if let Some(tf) = ds.trigger_frame {
                if tf < u64::MAX - 12 && self.frame_count >= tf {
                    ds.paused = true;
                    ds.trigger_frame = None;
                    ds.log(format!("⏸ Paused at frame {}", self.frame_count));
                }
                if tf >= u64::MAX - 12 {
                    let btn = (u64::MAX - tf) as usize;
                    if btn < 12 && self.callback_context.input_state[btn] {
                        ds.paused = true;
                        ds.trigger_frame = None;
                        ds.log(format!("⏸ Button trigger fired: btn={}", btn));
                    }
                }
            }

            if let Some((px, py)) = ds.trigger_pixel {
                if px < ds.fb_width && py < ds.fb_height && !ds.fb_rgba.is_empty() {
                    let idx = (py as usize * ds.fb_width as usize + px as usize) * 4;
                    if idx + 2 < ds.fb_rgba.len() {
                        let (r, g, b) = (ds.fb_rgba[idx], ds.fb_rgba[idx+1], ds.fb_rgba[idx+2]);
                        if r != 0 || g != 0 || b != 0 {
                            ds.paused = true;
                            ds.trigger_pixel = None;
                            ds.log(format!("⏸ Pixel trigger ({px},{py}) = #{r:02X}{g:02X}{b:02X}"));
                        }
                    }
                }
            }

            if ds.step_one {
                ds.step_one = false;
                false
            } else {
                ds.paused
            }
        };

        if paused { return Ok(false); }

        // --- Run emulation frame ---
        self.core
            .run()
            .map_err(|e| anyhow!("Core execution failed: {}", e))?;
        self.frame_count += 1;

        // --- Capture CPU state (fbalpha2012 debug API) ---
        self.capture_cpu_state();

        // --- Capture bookmark if requested ---
        self.maybe_capture_bookmark();

        // --- Save regions sidecar if requested ---
        let needs_save = self.debug_state.try_lock()
            .map(|mut ds| { let v = ds.save_regions; ds.save_regions = false; v })
            .unwrap_or(false);
        if needs_save {
            if let Some(ref path) = self.sidecar_path {
                save_regions_sidecar(path, &self.debug_state);
            }
        }

        // --- Apply pending AV info change ---
        if let Some(new_info) = self.callback_context.pending_av_info.take() {
            let av = new_info.to_rust();
            {
                let mut ds = self.debug_state.lock().unwrap();
                ds.av_width = av.geometry.base_width;
                ds.av_height = av.geometry.base_height;
                ds.fps = av.timing.fps;
                ds.log(format!("AV info updated: {}×{} @ {:.2}fps",
                    av.geometry.base_width, av.geometry.base_height, av.timing.fps));
            }
            self.av_info = Some(av);
        }

        // --- Update debug video counters ---
        {
            let ctx = &self.callback_context;
            let mut ds = self.debug_state.lock().unwrap();
            ds.video_frames = ctx.video_frames;
            ds.video_real = ctx.video_real;
        }

        Ok(self.callback_context.video_real > 0)
    }

    /// Borrow the current framebuffer: (data, width, height, pitch, pixel_format).
    pub fn framebuffer(&self) -> Option<(&[u8], u32, u32, usize, u32)> {
        let ctx = &self.callback_context;
        if ctx.framebuffer.is_empty() || ctx.width == 0 || ctx.height == 0 {
            None
        } else {
            Some((&ctx.framebuffer, ctx.width, ctx.height, ctx.pitch, ctx.pixel_format))
        }
    }

    /// Drain all queued audio samples (stereo interleaved i16).
    pub fn drain_audio(&mut self) -> Vec<i16> {
        std::mem::take(&mut self.callback_context.pending_audio)
    }

    pub fn shutdown(&self) {
        // Save regions sidecar before shutting down
        if let Some(ref path) = self.sidecar_path {
            save_regions_sidecar(path, &self.debug_state);
        }
        let _ = self.core.unload_game();
        let _ = self.core.deinit();
    }
}

impl Drop for Frontend {
    fn drop(&mut self) {
        CALLBACK_CONTEXT.store(std::ptr::null_mut(), Ordering::SeqCst);
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
                        *(data as *mut *const i8) = self.system_dir_buffer.as_ptr() as *const i8;
                        return true;
                    }
                    false
                }
                RETRO_ENVIRONMENT_GET_SAVE_DIRECTORY => {
                    if !data.is_null() {
                        *(data as *mut *const i8) = self.save_dir_buffer.as_ptr() as *const i8;
                        return true;
                    }
                    false
                }
                RETRO_ENVIRONMENT_GET_VARIABLE => false,
                RETRO_ENVIRONMENT_GET_VFS_INTERFACE => false,
                RETRO_ENVIRONMENT_GET_LOG_INTERFACE => {
                    if !data.is_null() {
                        unsafe extern "C" fn core_log(level: u32, msg: *const std::ffi::c_char) {
                            let prefix = match level { 0=>"[CORE DBG]", 1=>"[CORE INF]", 2=>"[CORE WRN]", _=>"[CORE ERR]" };
                            if !msg.is_null() {
                                let s = std::ffi::CStr::from_ptr(msg).to_string_lossy();
                                eprintln!("{} {}", prefix, s.trim_end());
                            }
                        }
                        (*(data as *mut RetroLogCallback)).log = core_log as *const std::ffi::c_void;
                        return true;
                    }
                    false
                }
                RETRO_ENVIRONMENT_GET_CORE_OPTIONS_VERSION => {
                    if !data.is_null() { *(data as *mut u32) = 0; }
                    true
                }
                RETRO_ENVIRONMENT_GET_VARIABLE_UPDATE => {
                    if !data.is_null() { *(data as *mut bool) = false; }
                    true
                }
                RETRO_ENVIRONMENT_GET_LANGUAGE => {
                    if !data.is_null() { *(data as *mut u32) = 0; }
                    true
                }
                RETRO_ENVIRONMENT_GET_AUDIO_VIDEO_ENABLE => {
                    if !data.is_null() { *(data as *mut i32) = 1 | 2; }
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
                RETRO_ENVIRONMENT_SHUTDOWN => { eprintln!("[CORE] Shutdown requested"); false }
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
                RETRO_ENVIRONMENT_SET_MEMORY_MAPS => {
                    if !data.is_null() {
                        self.handle_set_memory_maps(data as *const RetroMemoryMap);
                    }
                    true
                }
                RETRO_ENVIRONMENT_GET_CAN_DUPE => {
                    if !data.is_null() { *(data as *mut bool) = true; }
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

    fn handle_set_memory_maps(&mut self, map: *const RetroMemoryMap) {
        unsafe {
            if map.is_null() {
                return;
            }
            let map = *map;
            if map.descriptors.is_null() {
                return;
            }

            let mut regions = Vec::new();
            for i in 0..map.num_descriptors {
                let desc = &*map.descriptors.add(i as usize);
                // Stop at null ptr (sentinel)
                if desc.ptr.is_null() {
                    break;
                }

                let addr_start = desc.start;
                let addr_end = desc.start + desc.len - 1;
                let name = if !desc.addrspace.is_null() {
                    std::ffi::CStr::from_ptr(desc.addrspace)
                        .to_string_lossy()
                        .to_string()
                } else {
                    if desc.flags & crate::libretro::RETRO_MEMDESC_VIDEO_RAM != 0 {
                        "VRAM".to_string()
                    } else if desc.flags & crate::libretro::RETRO_MEMDESC_SAVE_RAM != 0 {
                        "SRAM".to_string()
                    } else if desc.flags & crate::libretro::RETRO_MEMDESC_SYSTEM_RAM != 0 {
                        "System RAM".to_string()
                    } else if desc.flags & crate::libretro::RETRO_MEMDESC_CONST != 0 {
                        "ROM".to_string()
                    } else {
                        "Memory".to_string()
                    }
                };

                let region = crate::debug::MemoryRegion {
                    name,
                    addr_start,
                    addr_end,
                    size: desc.len,
                    flags: desc.flags,
                    ptr: desc.ptr as usize,
                    offset: desc.offset,
                    select: desc.select,
                    disconnect: desc.disconnect,
                };
                regions.push(region);
            }

            if let Ok(mut ds) = self.debug_state.try_lock() {
                ds.memory_regions = regions;
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
    if ctx_ptr.is_null() { return false; }
    unsafe { (*ctx_ptr).environment_callback(cmd as u32, data) }
}

extern "C" fn static_video_callback(data: *const c_void, width: u32, height: u32, pitch: usize) {
    let ctx_ptr = CALLBACK_CONTEXT.load(Ordering::SeqCst);
    if !ctx_ptr.is_null() {
        unsafe { (*ctx_ptr).video_callback(data, width, height, pitch) };
    }
}

extern "C" fn static_input_poll_callback() {}

extern "C" fn static_input_state_callback(port: u32, device: u32, index: u32, id: u32) -> i16 {
    let ctx_ptr = CALLBACK_CONTEXT.load(Ordering::SeqCst);
    if ctx_ptr.is_null() { return 0; }
    unsafe { (*ctx_ptr).input_state_callback(port, device, index, id) }
}

extern "C" fn static_audio_callback(_left: i16, _right: i16) {}

extern "C" fn static_audio_batch_callback(data: *const i16, frames: usize) -> usize {
    let ctx_ptr = CALLBACK_CONTEXT.load(Ordering::SeqCst);
    if ctx_ptr.is_null() { return frames; }
    unsafe { (*ctx_ptr).audio_batch_callback(data, frames) }
}

/// Downsample an RGBA framebuffer (w×h) to (out_w×out_h) using nearest-neighbor.
/// Returns empty Vec if source is empty or dimensions are zero.
fn downsample_thumbnail(rgba: &[u8], w: u32, h: u32, out_w: u32, out_h: u32) -> Vec<u8> {
    if rgba.is_empty() || w == 0 || h == 0 { return Vec::new(); }
    let mut out = vec![0u8; (out_w * out_h * 4) as usize];
    for oy in 0..out_h {
        let sy = (oy * h / out_h) as usize;
        for ox in 0..out_w {
            let sx = (ox * w / out_w) as usize;
            let src_idx = (sy * w as usize + sx) * 4;
            let dst_idx = (oy as usize * out_w as usize + ox as usize) * 4;
            if src_idx + 3 < rgba.len() {
                out[dst_idx..dst_idx+4].copy_from_slice(&rgba[src_idx..src_idx+4]);
            }
        }
    }
    out
}

/// Sidecar file format — bookmarks and code regions for one ROM.
#[derive(serde::Serialize, serde::Deserialize)]
struct RegionsSidecar {
    bookmarks: Vec<crate::debug::Bookmark>,
    code_regions: Vec<crate::debug::CodeRegion>,
}

/// Load a regions sidecar file into debug state. Silently ignores missing files.
fn load_regions_sidecar(path: &std::path::Path, debug_state: &SharedDebugState) {
    let data = match std::fs::read_to_string(path) {
        Ok(d) => d,
        Err(_) => return, // file doesn't exist yet — that's fine
    };
    match serde_json::from_str::<RegionsSidecar>(&data) {
        Ok(sidecar) => {
            if let Ok(mut ds) = debug_state.lock() {
                let bm_count  = sidecar.bookmarks.len();
                let reg_count = sidecar.code_regions.len();
                ds.bookmarks    = sidecar.bookmarks;
                ds.code_regions = sidecar.code_regions;
                ds.log(format!("📂 Loaded {} bookmark(s) and {} region(s) from {}", bm_count, reg_count, path.display()));
            }
        }
        Err(e) => eprintln!("[Regions] Failed to parse sidecar {}: {}", path.display(), e),
    }
}

/// Save bookmarks and code regions to a JSON sidecar file (atomic write via .tmp).
fn save_regions_sidecar(path: &std::path::Path, debug_state: &SharedDebugState) {
    let (bookmarks, code_regions) = match debug_state.lock() {
        Ok(ds) => (ds.bookmarks.clone(), ds.code_regions.clone()),
        Err(_) => return,
    };
    let sidecar = RegionsSidecar { bookmarks, code_regions };
    let json = match serde_json::to_string_pretty(&sidecar) {
        Ok(j) => j,
        Err(e) => { eprintln!("[Regions] Serialization failed: {}", e); return; }
    };
    let tmp = path.with_extension("regions.json.tmp");
    if std::fs::write(&tmp, &json).is_ok() {
        if let Err(e) = std::fs::rename(&tmp, path) {
            eprintln!("[Regions] Failed to rename sidecar: {}", e);
        } else if let Ok(mut ds) = debug_state.lock() {
            ds.log(format!("💾 Saved regions to {}", path.display()));
        }
    } else {
        eprintln!("[Regions] Failed to write sidecar to {}", tmp.display());
    }
}
