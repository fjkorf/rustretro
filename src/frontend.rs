use crate::debug::{Bookmark, SharedDebugState};
use crate::libretro::*;
use anyhow::{anyhow, Result};
use std::ffi::{CString, c_uint, c_void};
use std::path::{Path, PathBuf};
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
    /// Where bus-window configs persist (`<save_dir>/<rom_stem>.busmap.json`,
    /// or the `--bus-map` override).
    busmap_path: Option<std::path::PathBuf>,
    /// Whether the core's Sek bus-read API worked: None = not yet tried,
    /// Some(false) = probed and absent (stop retrying — and stop re-running
    /// dlsym probes every frame), Some(true) = live.
    bus_bridge_ok: Option<bool>,
    /// Optional per-frame trace recorder (`--record`) for the shadow project.
    recorder: Option<crate::record::FrameRecorder>,
    /// Optional in-app shadow bot (`--shadow`): drives controller port 1 from
    /// a fitted kNN model, ticked from `run_frame` on the emu thread.
    shadow: Option<crate::shadow_runner::ShadowRunner>,
    /// Set when the shadow model changed (set_shadow / runtime load) so
    /// `drain_shadow_ops` republishes `shadow_model` once instead of cloning
    /// the info card every frame.
    shadow_info_dirty: bool,
    /// Where slot save-state files live (`<save_dir>/<rom_stem>.state<N>`).
    save_dir: PathBuf,
    /// ROM file stem used to name slot save-state files.
    rom_stem: Option<String>,
}

/// Resolve the on-disk path of save-state slot `slot` (1..=9) for a ROM:
/// `<save_dir>/<rom_stem>.state<N>` — the same sidecar naming convention as
/// `.regions.json` / `.busmap.json`. Pure so it's unit-testable without a core.
pub fn state_slot_path(save_dir: &std::path::Path, rom_stem: &str, slot: u8) -> PathBuf {
    save_dir.join(format!("{rom_stem}.state{slot}"))
}

impl Frontend {
    pub fn new(
        core_path: &str,
        rom_path: &str,
        save_dir: PathBuf,
        system_dir: PathBuf,
        debug_state: SharedDebugState,
        bus_map: Option<PathBuf>,
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

        // Derive the literate ROM-map path: library/<slug>/<slug>.md where the
        // slug is the ROM file stem (e.g. "mvsc" from "mvsc.zip"). We use the
        // project-root-relative `library/` directory (cwd-relative, matching how
        // .regions.json / rustretro_layout.json are resolved). The MCP
        // add_rom_map_region/get_rom_map tools read & scaffold this file.
        let rom_stem = std::path::Path::new(rom_path)
            .file_stem()
            .map(|s| s.to_string_lossy().to_string());
        let rom_map_path = rom_stem
            .as_ref()
            .map(|slug| PathBuf::from("library").join(slug).join(format!("{slug}.md")));

        // Busmap sidecar: explicit --bus-map wins; else <save_dir>/<rom_stem>.busmap.json
        let busmap_path = bus_map.or_else(|| {
            std::path::Path::new(rom_path)
                .file_stem()
                .map(|stem| save_dir.join(format!("{}.busmap.json", stem.to_string_lossy())))
        });

        let mut frontend = Frontend {
            core,
            av_info: None,
            callback_context,
            _game_path_cstring: None,
            frame_count: 0,
            debug_state: Arc::clone(&debug_state),
            sidecar_path: sidecar_path.clone(),
            did_memory_fallback: false,
            busmap_path: busmap_path.clone(),
            bus_bridge_ok: None,
            recorder: None,
            shadow: None,
            shadow_info_dirty: false,
            save_dir: save_dir.clone(),
            rom_stem: rom_stem.clone(),
        };

        // Store sidecar path in debug state and try to load existing data
        if let Ok(mut ds) = debug_state.lock() {
            ds.sidecar_path = sidecar_path.clone();
            ds.rom_map_path = rom_map_path.clone();
            ds.rom_name = rom_stem.clone();
            // Slot files live in save_dir; the State panel stats them via this.
            ds.state_dir = Some(save_dir.clone());
            // Infer the ROM-map `system` slug from the core (None for multi-system
            // cores like FBNeo, which the scaffold then leaves blank).
            ds.rom_system =
                crate::debug::system_slug_from_library(&system_info.library_name).map(String::from);
            // Keep the ROM path so the `rom_file` MCP source can re-read the cart
            // (needed for need_fullpath cores, which never load the bytes here).
            ds.rom_path = Some(std::path::PathBuf::from(rom_path));
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

        // Seed the ROM-map identity (frontmatter `rom.sha1`, §3) from the loaded
        // bytes, when available. Skipped for need_fullpath cores (no bytes here).
        if !rom_data.is_empty() {
            let sha1 = sha1_hex(&rom_data);
            let size = rom_data.len();
            if let Ok(mut ds) = debug_state.lock() {
                ds.rom_sha1 = Some(sha1);
                ds.rom_size = Some(size);
                // Retain the raw cart bytes for the `rom_file` MCP source (decodes
                // CHR-ROM graphics etc. the core won't expose). Cheap for the small
                // ROMs this path handles; need_fullpath cores skip it (empty here).
                ds.rom_bytes = Some(rom_data.clone());
            }
        }

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

        // Install bus windows from the busmap sidecar (Sek snapshot bridge).
        // Buffers stay zero-filled until the first post-run refresh.
        if let Some(ref path) = busmap_path {
            load_busmap_sidecar(path, &debug_state);
        }

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
            // Bus-window regions don't count: they're our own synthesis, not a
            // core-published map, and must not suppress this fallback.
            if ds.has_non_bus_regions() {
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
            if ds.has_non_bus_regions() {
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
            // Extend, don't assign: bus-window regions may already be installed.
            ds.memory_regions.extend(synthesized);
        }
        // Only stop retrying once we've actually synthesized something (or the
        // core published a map — handled above). A null get_memory_data at
        // load time leaves did_memory_fallback false so we retry next frame.
        self.did_memory_fallback = true;
    }

    /// Install bus windows queued by the MCP `map_bus_window` tool, then give
    /// just the new ones an immediate one-shot snapshot so they hold real data
    /// even if the emulator is paused.
    fn drain_pending_bus_windows(&mut self) {
        let first_new = {
            let mut ds = match self.debug_state.lock() {
                Ok(ds) => ds,
                Err(_) => return,
            };
            if ds.pending_bus_windows.is_empty() {
                return;
            }
            let first_new = ds.bus_windows.len();
            let pending = std::mem::take(&mut ds.pending_bus_windows);
            for cfg in pending {
                let desc = format!("{} @ 0x{:X}+0x{:X}", cfg.name, cfg.addr, cfg.len);
                if ds.install_bus_window(cfg) {
                    ds.log(format!("[bus] mapped window {desc}"));
                } else {
                    ds.log(format!("[bus] rejected window {desc} (empty or name taken)"));
                }
            }
            first_new
        };
        self.refresh_bus_windows(Some(first_new));
    }

    /// Snapshot bus windows into their owned buffers via the core's exported
    /// Sek read API. `only_from = Some(i)` refreshes windows `i..` regardless
    /// of interval (one-shot fill of freshly installed windows); `None` is the
    /// per-frame path honoring each window's `interval`.
    ///
    /// All FFI happens WITHOUT the DebugState lock (the emu thread owns the
    /// core; readers only ever see the buffers), then one short try_lock
    /// publishes the bytes. On lock contention the publish is skipped — the
    /// snapshot is simply one frame staler.
    fn refresh_bus_windows(&mut self, only_from: Option<usize>) {
        if self.bus_bridge_ok == Some(false) {
            return;
        }
        let windows: Vec<(usize, crate::debug::BusWindowCfg)> = match self.debug_state.lock() {
            Ok(ds) => ds
                .bus_windows
                .iter()
                .enumerate()
                .filter(|(i, w)| match only_from {
                    Some(start) => *i >= start,
                    None => self.frame_count % (w.interval.max(1) as u64) == 0,
                })
                .map(|(i, w)| (i, w.clone()))
                .collect(),
            Err(_) => return,
        };
        if windows.is_empty() {
            return;
        }

        let mut fetched: Vec<(usize, Vec<u8>)> = Vec::with_capacity(windows.len());
        for (i, w) in &windows {
            match self.core.sek_read_block(w.addr, w.len as usize) {
                Some(bytes) => fetched.push((*i, bytes)),
                None => {
                    // Core has no (guarded) Sek API — it never will; stop
                    // probing every frame and say so once.
                    self.bus_bridge_ok = Some(false);
                    if let Ok(mut ds) = self.debug_state.lock() {
                        ds.log(
                            "[bus] core does not export the Sek bus-read API; \
                             bus windows will stay zero-filled"
                                .to_string(),
                        );
                    }
                    return;
                }
            }
        }
        if self.bus_bridge_ok.is_none() {
            self.bus_bridge_ok = Some(true);
        }

        if let Ok(mut ds) = self.debug_state.try_lock() {
            for (i, bytes) in fetched {
                if let Some(buf) = ds.bus_buffers.get_mut(i) {
                    if buf.len() == bytes.len() {
                        buf.copy_from_slice(&bytes);
                    }
                }
            }
        }
    }

    /// Push queued bus-window writes (from `write_addr` / freeze / MCP
    /// write_memory) to the live 68k bus via `sek_write_block`. Takes the queue
    /// under a brief lock, then does the FFI unlocked — same discipline as
    /// `refresh_bus_windows`. Skipped when the core has no Sek bridge.
    fn drain_bus_writes(&mut self) {
        if self.bus_bridge_ok == Some(false) {
            return;
        }
        let writes: Vec<(u32, Vec<u8>)> = match self.debug_state.try_lock() {
            Ok(mut ds) => std::mem::take(&mut ds.pending_bus_writes),
            Err(_) => return,
        };
        for (addr, bytes) in writes {
            self.core.sek_write_block(addr, &bytes);
        }
    }

    /// Resolve a [`StateOp`](crate::debug::StateOp) slot number to its file path.
    fn slot_path(&self, slot: u8) -> PathBuf {
        let stem = self.rom_stem.as_deref().unwrap_or("game");
        state_slot_path(&self.save_dir, stem, slot)
    }

    /// Serialize the core and atomically write the state to `path` (.tmp+rename,
    /// same discipline as the sidecar files). Returns the state size in bytes.
    fn do_save_state(&self, path: &std::path::Path) -> Result<usize, String> {
        let bytes = self
            .core
            .serialize()
            .ok_or_else(|| "core refused to serialize (no save-state support?)".to_string())?;
        // Arena captures land in per-family dirs (shadow/arenas/<family>/)
        // that don't exist until a game's first save — create, don't fail.
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("create dir {} failed: {e}", dir.display()))?;
        }
        let tmp = PathBuf::from(format!("{}.tmp", path.display()));
        std::fs::write(&tmp, &bytes)
            .map_err(|e| format!("write {} failed: {e}", tmp.display()))?;
        std::fs::rename(&tmp, path)
            .map_err(|e| format!("rename to {} failed: {e}", path.display()))?;
        Ok(bytes.len())
    }

    /// Read a state file and hand it to the core. Returns the state size.
    fn do_load_state(&self, path: &std::path::Path) -> Result<usize, String> {
        let bytes = std::fs::read(path)
            .map_err(|e| format!("read {} failed: {e}", path.display()))?;
        let expect = self.core.serialize_size();
        if self.core.unserialize(&bytes) {
            Ok(bytes.len())
        } else {
            Err(format!(
                "core rejected the state ({} bytes; core expects {expect})",
                bytes.len()
            ))
        }
    }

    /// Snapshot the fighter blocks + gate + run the liveness probe, then
    /// write `<name>.meta.json` next to `path` (mission: make arenas
    /// self-describing — see [`ArenaMeta`]). Best-effort: a lock or I/O
    /// failure just skips the sidecar, never fails the save itself.
    fn write_arena_sidecar(&mut self, path: &Path) {
        let profile = crate::profile::current();
        let (gate_open, char1, char2, health1, health2) = match self.debug_state.lock() {
            Ok(ds) => {
                let gate_open = crate::gate::eval_gate(&ds, profile);
                let rd = |ba: Option<(u32, u8)>| {
                    ba.and_then(|(a, s)| ds.read_addr(a as usize, s as usize))
                };
                let char1 = rd(profile.field_addr(1, "char_id")).map(|v| profile.canon_char_id(v as u8));
                let char2 = rd(profile.field_addr(2, "char_id")).map(|v| profile.canon_char_id(v as u8));
                let health1 = rd(profile.field_addr(1, "health")).map(|v| v as u8);
                let health2 = rd(profile.field_addr(2, "health")).map(|v| v as u8);
                (gate_open, char1, char2, health1, health2)
            }
            Err(_) => return,
        };

        let inputs_live = self.probe_arena_liveness(path, profile, gate_open);

        let meta = ArenaMeta {
            format: "arena-meta-v1".to_string(),
            family: profile.family.family.clone(),
            port: profile.port.port.clone(),
            char_id_block1: char1,
            char_id_block2: char2,
            health_block1: health1,
            health_block2: health2,
            gate_open,
            inputs_live,
            saved_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        };
        let meta_path = path.with_extension("meta.json");
        match serde_json::to_string_pretty(&meta) {
            Ok(json) => match std::fs::write(&meta_path, json) {
                Ok(()) => {
                    if let Ok(mut ds) = self.debug_state.lock() {
                        ds.log(format!("🏟 Arena sidecar written: {}", meta_path.display()));
                    }
                }
                Err(e) => eprintln!("[arena] failed to write sidecar {}: {e}", meta_path.display()),
            },
            Err(e) => eprintln!("[arena] sidecar serialization failed: {e}"),
        }
    }

    /// The liveness probe (mission task 2, extended by task B-fix for
    /// `via: "object_ptr"` ports): empirically determine whether controller
    /// ports 0/1 each drive a fighter in the state just saved to `path`.
    /// Degrades honestly to `None` (unknown) — never guesses — when a
    /// recording is active, the gate is closed, or the profile can't resolve
    /// `x` for a block AT ALL, fixed or pointer-resolved (see
    /// [`liveness_precheck`], [`x_is_readable`]).
    ///
    /// Every side effect is undone before returning: pause state, the
    /// training enforcer's `enabled` flag, and the shadow bot's `enabled`
    /// flag are restored, the frame counter is rewound, and — the actual
    /// "leaves no trace" mechanism — the exact bytes just saved to `path`
    /// are reloaded, discarding whatever the probe's injected input did to
    /// the live RAM.
    fn probe_arena_liveness(
        &mut self,
        path: &Path,
        profile: &crate::profile::GameProfile,
        gate_open: bool,
    ) -> InputsLive {
        let has_x1 = x_is_readable(profile, 1);
        let has_x2 = x_is_readable(profile, 2);
        if let Some(skip) = liveness_precheck(self.recorder.is_some(), gate_open, has_x1, has_x2) {
            return skip;
        }

        let frame_count_before = self.frame_count;
        let (was_paused, was_training) = match self.debug_state.lock() {
            Ok(mut ds) => {
                let p = ds.paused;
                let t = ds.training.enabled;
                ds.paused = false;
                // Disable the training enforcer / dummy for the probe's
                // duration — its position resets and dummy AI would
                // confound "did MY injected input move this fighter".
                ds.training.enabled = false;
                (p, t)
            }
            Err(_) => return InputsLive::default(),
        };
        let was_shadow_enabled = self.shadow.as_ref().map(|s| s.enabled);
        if let Some(sh) = self.shadow.as_mut() {
            // Same reason: the shadow bot also drives port 1.
            sh.enabled = false;
        }

        // Block 1 ↔ port 0, block 2 ↔ port 1 (the 2-human-rig convention
        // this probe assumes — same pairing the round anchor uses).
        let p0 = has_x1.then(|| self.probe_one_port(0, profile, 1)).flatten();
        let p1 = has_x2.then(|| self.probe_one_port(1, profile, 2)).flatten();

        // Leave no trace: reload the exact bytes just saved, clear any input
        // still "held" from the probe, and restore every flag we touched.
        let _ = self.do_load_state(path);
        self.refresh_bus_windows(Some(0));
        self.set_input([false; 12]);
        self.set_input2([false; 12]);
        self.frame_count = frame_count_before;
        if let Ok(mut ds) = self.debug_state.lock() {
            ds.paused = was_paused;
            ds.training.enabled = was_training;
        }
        if let Some(sh) = self.shadow.as_mut() {
            sh.enabled = was_shadow_enabled.unwrap_or(false);
        }

        InputsLive { p0, p1 }
    }

    /// Drive `port` (0 or 1) with an alternating movement probe and report
    /// whether `block`'s fighter (1 or 2 — the block this port is assumed to
    /// drive) shows real motion beyond what the SAME window shows with no
    /// input at all (see [`judge_burst_motion_differential`]) — never a
    /// single before/after read, a symmetric round-trip, or an absolute
    /// "did it move" test.
    ///
    /// Longer and more careful than the naive version on purpose. A live-RE
    /// session on this exact field (mk2.md, "Input-liveness verification")
    /// found that a short before/after window is unreliable two ways — (a)
    /// it can land entirely inside hitstun/knockdown, where `x` is
    /// genuinely frozen despite live, landing input, and (b) some ports'
    /// `x` lives in a dynamic object-pool slot that can itself go stale
    /// across boots — resolved live here through `GameProfile::object_ptr_field`
    /// (docs/frames.md §5) exactly like the recorder and training dummy, so
    /// a stale/invalid pointer on a given sampled frame just yields a
    /// missing sample rather than a wrong one.
    ///
    /// A THIRD failure mode, found live while verifying task B-fix against
    /// `shadow/arenas/mk2/reptile-vs-reptile.state` (a documented 1P-vs-CPU
    /// rig, mk2.md "Arena recapture"): an absolute "did it move while I held
    /// a direction" test reports the CPU-driven block's port as live too,
    /// because the CPU's OWN autonomous movement satisfies the same
    /// threshold regardless of injected input — exactly the docs/frames.md
    /// §4.2 warning about absolute observables during "any scripted motion".
    /// The fix mirrors that section's mandatory differential form: snapshot
    /// the exact starting state, measure a CONTROL run with NO input on
    /// either port (`run_liveness_window` with `inject_port: None`), restore
    /// the identical snapshot, then measure the PROBE run with the
    /// alternating hold — only a margin the control run didn't already show
    /// counts as live, which cancels out autonomous/AI motion the same way
    /// a differential probe cancels pushback and hitstop.
    ///
    /// Within each run, bursts are judged per-direction from their own
    /// start→latest delta — walk speeds are measured ASYMMETRIC on MK2
    /// (+12px/6f forward vs +5px/6f backward, docs/frames.md §5), so a
    /// shared magnitude after a round trip would produce false negatives on
    /// the slower direction.
    ///
    /// Returns `None` — honestly unknown, never a guessed `false` — when not
    /// one sample resolved in the PROBE run (e.g. an object-pointer port
    /// whose pointer never validated).
    fn probe_one_port(&mut self, port: u8, profile: &crate::profile::GameProfile, block: u8) -> Option<bool> {
        const BURST_FRAMES: u32 = 20; // direction flip cadence — avoids a wall/corner false-negative

        let little = profile.port.memory.endianness == "little";
        let is_obj = profile.field_is_object_ptr("x");
        let fixed = (!is_obj).then(|| profile.field_addr(block, "x")).flatten();
        // Same size-scaled floor as the original single-shot spread check
        // (8 for a 2-byte field, 4 for 1 byte) — now applied per burst, and
        // to the PROBE-minus-CONTROL excess rather than a raw delta.
        let field_size: u8 = if is_obj {
            profile
                .port
                .memory
                .fighter_fields
                .iter()
                .find(|f| f.name == "x")
                .map(|f| f.size)
                .unwrap_or(2)
        } else {
            fixed.map(|(_, s)| s).unwrap_or(2)
        };
        let delta_threshold: i64 = if field_size >= 2 { 8 } else { 4 };

        // Snapshot → CONTROL (no input anywhere) → restore → PROBE (the
        // alternating hold on `port`) → restore. Both runs see an IDENTICAL
        // starting state, so any difference between them is attributable to
        // the injected input, not autonomous CPU behavior (which a
        // deterministic core reproduces identically from the same bytes).
        let snapshot = self.core.serialize();
        let control = self.run_liveness_window(profile, block, is_obj, fixed, little, BURST_FRAMES, None);
        if let Some(bytes) = &snapshot {
            self.core.unserialize(bytes);
            self.refresh_bus_windows(Some(0));
        }
        let probe =
            self.run_liveness_window(profile, block, is_obj, fixed, little, BURST_FRAMES, Some(port));
        if let Some(bytes) = &snapshot {
            self.core.unserialize(bytes);
            self.refresh_bus_windows(Some(0));
        }

        judge_burst_motion_differential(&probe, &control, BURST_FRAMES as usize, delta_threshold)
    }

    /// Runs one 180-frame liveness-probe window, forcing both fighters'
    /// health up every frame (removes the KO/hitstun confound) and, when
    /// `inject_port` is `Some`, holding the alternating left/right burst on
    /// that controller port. `inject_port: None` is the CONTROL pass (no
    /// input on either port) — same burst/direction bookkeeping as a probe
    /// pass so the two line up 1:1 for [`judge_burst_motion_differential`].
    /// Samples `block`'s `x` every frame, live-resolving through
    /// `GameProfile::object_ptr_field` when `is_obj` (docs/frames.md §5).
    fn run_liveness_window(
        &mut self,
        profile: &crate::profile::GameProfile,
        block: u8,
        is_obj: bool,
        fixed: Option<(u32, u8)>,
        little: bool,
        burst_frames: u32,
        inject_port: Option<u8>,
    ) -> Vec<(bool, Option<i64>)> {
        const TOTAL_FRAMES: u32 = 180; // ~3s emulated at 60fps
        let health_max = profile.port.enforcement.health_max;
        let h1 = profile.field_addr(1, "health");
        let h2 = profile.field_addr(2, "health");
        let right = crate::profile::retro_button_bit("right");
        let left = crate::profile::retro_button_bit("left");

        let mut samples: Vec<(bool, Option<i64>)> = Vec::with_capacity(TOTAL_FRAMES as usize);
        for frame in 0..TOTAL_FRAMES {
            if let Ok(mut ds) = self.debug_state.lock() {
                if let Some((a, s)) = h1 {
                    ds.write_addr(a as usize, s as usize, health_max as u32);
                }
                if let Some((a, s)) = h2 {
                    ds.write_addr(a as usize, s as usize, health_max as u32);
                }
            }
            let going_right = (frame / burst_frames) % 2 == 0;
            if let Some(port) = inject_port {
                let mut bits = [false; 12];
                if let Some(bit) = if going_right { right } else { left } {
                    bits[bit as usize] = true;
                }
                if port == 0 {
                    self.set_input(bits);
                } else {
                    self.set_input2(bits);
                }
            }
            let _ = self.run_frame();

            let sample: Option<i64> = match self.debug_state.lock() {
                Ok(ds) if is_obj => profile.object_ptr_field(block, "x", |addr, size| {
                    endian_fix(ds.read_addr(addr as usize, size as usize).unwrap_or(0), size, little)
                }),
                Ok(ds) => fixed.and_then(|(a, s)| ds.read_addr(a as usize, s as usize)).map(|v| v as i64),
                Err(_) => None,
            };
            samples.push((going_right, sample));
        }
        samples
    }

    /// Drain a queued save/load-state request (hotkeys / --load-state / MCP).
    ///
    /// SAFETY / PLACEMENT: retro_serialize and retro_unserialize must run on the
    /// emu thread BETWEEN complete retro_run calls — never mid-frame. This is
    /// called from two such points in `run_frame`: (a) right after `core.run()`
    /// + `drain_bus_writes()` (so a save captures this frame's queued pokes,
    /// and before `refresh_bus_windows` so a load's restored RAM is what gets
    /// snapshotted), and (b) on the paused early-return path (the previous
    /// frame is complete, so serialization is equally safe — without this an
    /// MCP save/load while paused would hang until resume).
    ///
    /// After a successful LOAD it forces a full bus-window refresh
    /// (`refresh_bus_windows(Some(0))`, ignoring per-window intervals) so the
    /// debugger/recorder see the restored RAM immediately.
    ///
    /// ATOMIC load-and-pause (docs/frames.md §4.6): when the queued op came
    /// in with `pending_state_op_pause_after` set, a successfully-drained
    /// LOAD forces `paused = true` in the SAME lock acquisition that
    /// publishes `state_op_result` below — see [`state_op_forces_pause`]
    /// for why that single critical section is sufficient (no
    /// `fold_generation`-style condvar dance needed here) and
    /// `RetroMcpServer::load_state`'s doc for the caller-facing contract.
    /// `pause_after` is taken out of `DebugState` in the SAME `try_lock` as
    /// `pending_state_op` itself, so the two can never be observed
    /// separately by a racing submitter.
    fn drain_state_op(&mut self) {
        let (op, pause_after) = match self.debug_state.try_lock() {
            Ok(mut ds) => (
                ds.pending_state_op.take(),
                std::mem::take(&mut ds.pending_state_op_pause_after),
            ),
            Err(_) => return,
        };
        let Some(op) = op else { return };

        use crate::debug::StateOp;
        // `is_named_save` distinguishes an explicit-path Save (MCP
        // save_state with a path, the Arena panel, the State panel's path
        // box) from a numbered slot save (F6 / quick-save) — only the
        // former can be an arena path, and slot saves must NEVER trigger
        // the sidecar/probe below.
        let (is_load, path, is_named_save) = match op {
            StateOp::Save(p) => (false, p, true),
            StateOp::Load(p) => (true, p, false),
            StateOp::SaveSlot(n) => (false, self.slot_path(n), false),
            StateOp::LoadSlot(n) => (true, self.slot_path(n), false),
        };

        let result = if is_load {
            self.do_load_state(&path)
        } else {
            self.do_save_state(&path)
        };

        if is_load && result.is_ok() {
            // Snapshot ALL bus windows now so readers see the restored RAM
            // this frame, not up to `interval` frames later.
            self.refresh_bus_windows(Some(0));
        }

        let verb = if is_load { "loaded" } else { "saved" };
        let published = match &result {
            Ok(bytes) => {
                eprintln!("[state] {verb} {} ({bytes} bytes)", path.display());
                Ok(crate::debug::StateOpDone {
                    loaded: is_load,
                    path: path.clone(),
                    bytes: *bytes,
                })
            }
            Err(e) => {
                eprintln!(
                    "[state] {} {} FAILED: {e}",
                    if is_load { "load" } else { "save" },
                    path.display()
                );
                Err(e.clone())
            }
        };
        if let Ok(mut ds) = self.debug_state.lock() {
            match &published {
                Ok(done) => ds.log(format!(
                    "💾 State {verb}: {} ({} bytes)",
                    done.path.display(),
                    done.bytes
                )),
                Err(e) => ds.log(format!("💾 State op failed: {e}")),
            }
            // Sticky copy for the State panel (state_op_result is consumed by
            // the MCP poller, so the GUI must not rely on reading it).
            ds.state_note = Some(match &published {
                Ok(done) => format!(
                    "{verb} {} ({} bytes) @ frame {}",
                    done.path.display(), done.bytes, self.frame_count
                ),
                Err(e) => format!(
                    "{} {} FAILED: {e}",
                    if is_load { "load" } else { "save" },
                    path.display()
                ),
            });
            // Atomic load-and-pause: force it in THIS lock acquisition, the
            // same one that's about to publish `state_op_result` a line
            // below — any thread that later locks `debug_state` to read the
            // result observes `paused == true` together with it, never a
            // result without the pause or a pause without the result (see
            // `state_op_forces_pause`'s doc for why a single critical
            // section suffices instead of a fold_generation-style wait).
            if state_op_forces_pause(pause_after, is_load, published.is_ok()) {
                ds.paused = true;
            }
            ds.state_op_result = Some(published);
        }

        // Arena self-description: write `<name>.meta.json` next to every
        // state saved to an `arenas/` path (never for slot saves, never for
        // loads — see `is_arena_path`). Runs the empirical port-liveness
        // probe (advances real frames, then reloads to leave no trace).
        // Deliberately AFTER `state_op_result` is published above, so an MCP
        // `save_state` roundtrip returns as soon as the save itself lands —
        // the probe (up to a few hundred extra frames) then runs without
        // making that caller wait on it.
        if is_named_save && !is_load && result.is_ok() && is_arena_path(&path) {
            self.write_arena_sidecar(&path);
        }
    }

    /// Drain GUI/MCP shadow requests (toggle + runtime model load — the panel
    /// only has `DebugState`, not `&mut Frontend`) and publish status back.
    /// Called at both `drain_state_op` sites so both work while paused too.
    ///
    /// Runtime load differs from the fatal `--shadow` startup path: a bad
    /// model becomes a note and the previous model (if any) keeps running.
    /// Enable policy on load: switching models preserves the current
    /// enabled state (keep fighting); arming the FIRST model starts disabled
    /// so a mid-round load doesn't grab port 1 unasked.
    fn drain_shadow_ops(&mut self) {
        let (load_req, want_toggle) = match self.debug_state.try_lock() {
            Ok(mut ds) => (
                ds.pending_shadow_load.take(),
                std::mem::take(&mut ds.pending_shadow_toggle),
            ),
            Err(_) => return,
        };

        let mut load_result: Option<Result<String, String>> = None;
        if let Some(dir) = load_req {
            let prev_enabled = self.shadow.as_ref().map(|s| s.enabled);
            match crate::shadow_runner::ShadowRunner::load(&dir) {
                Ok(mut runner) => {
                    runner.enabled = prev_enabled.unwrap_or(false);
                    let msg = format!(
                        "loaded {} ({} cases{}) — {}",
                        runner.info().name,
                        runner.info().cases,
                        runner.info().rounds.map_or(String::new(), |r| format!(", {r} rounds")),
                        if runner.enabled { "ACTIVE" } else { "press Enable / Shift+F5 to fight" },
                    );
                    self.shadow = Some(runner);
                    self.shadow_info_dirty = true;
                    load_result = Some(Ok(msg));
                }
                Err(e) => {
                    load_result = Some(Err(format!("load {} FAILED: {e}", dir.display())));
                }
            }
        }
        if want_toggle {
            self.toggle_shadow();
        }

        let on = self.shadow.as_ref().map(|s| s.enabled);
        let info = if self.shadow_info_dirty {
            Some(self.shadow.as_ref().map(|s| s.info().clone()))
        } else {
            None
        };
        if let Ok(mut ds) = self.debug_state.try_lock() {
            ds.shadow_on = on;
            if let Some(model) = info {
                ds.shadow_model = model;
                self.shadow_info_dirty = false;
            }
            if let Some(res) = load_result {
                let note = match &res {
                    Ok(m) => m.clone(),
                    Err(e) => e.clone(),
                };
                ds.log(format!("👤 Shadow: {note}"));
                ds.shadow_note = Some(note);
                ds.shadow_load_result = Some(res);
            }
        }
    }

    /// Drain GUI recorder start/stop requests and publish recording status
    /// (path + frames) every call, so the panel's counter stays live.
    fn drain_record_ops(&mut self) {
        let req = match self.debug_state.try_lock() {
            Ok(mut ds) => ds.pending_record.take(),
            Err(_) => return,
        };
        let mut note: Option<String> = None;
        match req {
            Some(crate::debug::RecordControl::Start { path, style }) => {
                if let Some(rec) = self.recorder.as_ref() {
                    note = Some(format!(
                        "already recording to {} — stop first",
                        rec.path().display()
                    ));
                } else {
                    self.set_recorder(path.clone(), style);
                    note = Some(if self.recorder.is_some() {
                        format!("recording → {}", path.display())
                    } else {
                        // set_recorder printed the reason to stderr
                        // (stub profile, or the file could not open).
                        format!("start FAILED — see stderr ({})", path.display())
                    });
                }
            }
            Some(crate::debug::RecordControl::Stop) => {
                if let Some(mut rec) = self.recorder.take() {
                    rec.finish();
                    note = Some(format!(
                        "stopped — {} frames → {}",
                        rec.frames_written(),
                        rec.path().display()
                    ));
                    eprintln!("[record] {}", note.as_deref().unwrap_or(""));
                } else {
                    note = Some("not recording".to_string());
                }
            }
            None => {}
        }
        let status = self
            .recorder
            .as_ref()
            .map(|r| (r.path().to_path_buf(), r.frames_written()));
        if let Ok(mut ds) = self.debug_state.try_lock() {
            ds.record_status = status;
            if let Some(n) = note {
                ds.log(format!("⏺ Record: {n}"));
                ds.record_note = Some(n);
            }
        }
    }

    /// Enable per-frame trace recording to `path` (jsonl-v3,
    /// `shadow/RECORDER_V3.md`). v3 is profile-driven: it needs the fighter
    /// blocks (schema-guaranteed once a profile parses) plus a non-empty
    /// controllable gate — a partially-mapped game (library/mk2) records
    /// honestly sparse rows instead of refusing. Warns if no readable region
    /// covers the fighter blocks (rows would read as zeros).
    pub fn set_recorder(&mut self, path: PathBuf, style: Option<String>) {
        let prof = crate::profile::current();
        // No gate list = no `controllable`, no round edges — refuse softly;
        // the drain publishes the note to the panel.
        if prof.port.gate.is_empty() {
            eprintln!(
                "[record] unavailable for this game: profile maps no `gate` conditions \
                 — v3 cannot resolve `controllable`/rounds without one (stub profile?)"
            );
            return;
        }
        let blocks_lo = prof.block1().min(prof.block2()) as usize;
        let blocks_hi = (prof.block1().max(prof.block2())
            + prof.port.memory.blocks.stride.0) as usize;
        let blocks_readable = self
            .debug_state
            .lock()
            .map(|ds| ds.memory_regions.iter().any(|r| {
                r.addr_start <= blocks_lo && r.addr_end >= blocks_hi
            }))
            .unwrap_or(false);
        if !blocks_readable {
            eprintln!(
                "[record] warning: no readable region covers the fighter blocks \
                 (0x{blocks_lo:X}..0x{blocks_hi:X}); rows will read as zeros. \
                 Pass --bus-map with a Work RAM window."
            );
        }
        match crate::record::FrameRecorder::create(
            &path,
            prof,
            &prof.port.core.provenance_game,
            &prof.port.core.provenance_core,
            style.as_deref(),
        ) {
            Ok(rec) => {
                eprintln!("[record] tracing per-frame to {}", path.display());
                self.recorder = Some(rec);
            }
            Err(e) => eprintln!("[record] failed to open {}: {e}", path.display()),
        }
    }

    /// Install the in-app shadow bot (`--shadow`). It arrives already enabled;
    /// Shift+F5 (→ [`toggle_shadow`](Self::toggle_shadow)) flips it later.
    pub fn set_shadow(&mut self, runner: crate::shadow_runner::ShadowRunner) {
        self.shadow = Some(runner);
        self.shadow_info_dirty = true;
    }

    /// Shift+F5: toggle the shadow bot on/off. With no model loaded, prints a
    /// hint naming the `--shadow` flag instead.
    pub fn toggle_shadow(&mut self) {
        match self.shadow.as_mut() {
            Some(sh) => sh.toggle(self.frame_count),
            None => eprintln!(
                "[shadow] no model loaded — launch with --shadow shadow/models/<name> \
                 to arm the in-app shadow bot"
            ),
        }
    }

    /// Append this frame to the trace, if recording. Called at the end of
    /// `run_frame` after the bus snapshot is refreshed (so actor fields are
    /// current). P1/P2 input masks come from the callback context — the
    /// authoritative RETRO joypad words for this frame.
    fn record_frame(&mut self) {
        if self.recorder.is_none() {
            return;
        }
        let p1 = crate::record::pack_mask(&self.callback_context.input_state);
        let p2 = crate::record::pack_mask(&self.callback_context.input_state2);
        if let Ok(ds) = self.debug_state.lock() {
            if let Some(rec) = self.recorder.as_mut() {
                rec.record(&ds, p1, p2);
            }
        }
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

    /// Set controller port 1 (P2) button states for the next frame.
    pub fn set_input2(&mut self, state: [bool; 12]) {
        self.callback_context.input_state2 = state;
    }

    /// Capture M68K and Z80 CPU state from the core (fbalpha2012-specific).
    fn capture_cpu_state(&self) {
        // The Sek debug API exists in cores (FBNeo) for EVERY game, but only
        // a 68k driver initializes its context — calling it under e.g. a
        // TMS34010 driver segfaults. Gate on the profile's declared CPU.
        if crate::profile::current().port.memory.cpu != "m68k" {
            return;
        }
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

        // Install windows the MCP thread queued, and give each an immediate
        // one-shot fill. This runs BEFORE the paused check so a paused session
        // that maps a window still gets data instead of zeros.
        self.drain_pending_bus_windows();

        // --- Check pause / triggers ---
        let paused = {
            let mut ds = self.debug_state.lock().unwrap();
            ds.push_input(self.callback_context.input_state, self.frame_count);
            // Mirror port 1 too so Lua `input.get(1)` has a cheap read.
            ds.input_state2 = self.callback_context.input_state2;
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

            let will_bail = if ds.step_one {
                ds.step_one = false;
                false
            } else if ds.step_batch_remaining > 0 {
                ds.step_batch_remaining -= 1;
                false
            } else {
                ds.paused
            };
            // Sticky record of what THIS frame fed the core, captured only
            // when we've just decided (in this same lock acquisition) that
            // the frame will actually run — see `last_executed_input`'s doc
            // for why this has to be atomic with the decision, not a
            // separate later write.
            if !will_bail {
                ds.last_executed_input = self.callback_context.input_state;
                ds.last_executed_input2 = self.callback_context.input_state2;
            }
            will_bail
        };

        if paused {
            // Between-frames is a safe serialization point too (see
            // drain_state_op) — service save/load here so a paused session's
            // MCP/hotkey state ops don't hang until resume.
            self.drain_state_op();
            self.drain_shadow_ops();
            self.drain_record_ops();
            return Ok(false);
        }

        // --- Run emulation frame ---
        self.core
            .run()
            .map_err(|e| anyhow!("Core execution failed: {}", e))?;
        self.frame_count += 1;

        // --- Capture CPU state (fbalpha2012 debug API) ---
        self.capture_cpu_state();

        // --- Push queued bus-window writes to the live bus (Sek bridge) ---
        // MUST run AFTER capture_cpu_state (which re-applies freeze writes into
        // pending_bus_writes) and BEFORE refresh_bus_windows (which re-snapshots),
        // so a frozen/poked value lands on the live bus and the snapshot then
        // reads it back — i.e. the freeze actually sticks.
        self.drain_bus_writes();

        // --- Save/load state (queued by hotkeys / --load-state / MCP) ---
        // AFTER core.run + drain_bus_writes (a save captures this frame's
        // settled state, including queued pokes) and BEFORE refresh_bus_windows
        // (a load's restored RAM is what gets snapshotted; the drain also forces
        // a full refresh itself on load). See drain_state_op for the safety
        // argument (serialize/unserialize only between complete retro_run calls).
        self.drain_state_op();
        self.drain_shadow_ops();
        self.drain_record_ops();

        // --- Refresh bus-window snapshots (Sek bridge) ---
        self.refresh_bus_windows(None);

        // --- Append this frame to the trace recorder (--record) ---
        self.record_frame();

        // --- Profile pins: hold declared RAM values for the whole session
        // (game settings that live in volatile RAM — e.g. MK2 Genesis's
        // per-port 6-button flags). Once a second; independent of training
        // and the gate because these matter in menus too.
        if self.frame_count % 60 == 0 {
            let pins = crate::profile::current().resolved_pins();
            if !pins.is_empty() {
                if let Ok(mut ds) = self.debug_state.try_lock() {
                    for (addr, value) in pins {
                        let _ = ds.write_addr(addr as usize, 1, value as u32);
                    }
                }
            }
        }

        // --- Training mode (--training): enforce sandbox + drive the dummy.
        // After the snapshot refresh so reads see this frame; its bus writes
        // drain next frame.
        if let Ok(mut ds) = self.debug_state.try_lock() {
            crate::training::tick(&mut ds, self.frame_count);
        }

        // --- Shadow bot (--shadow): decide every P frames while the fight
        // gate is open and inject the sampled intent on port 1. Runs AFTER
        // training::tick so its injection wins over a non-Free dummy preset.
        if let Some(sh) = self.shadow.as_mut() {
            if let Ok(mut ds) = self.debug_state.try_lock() {
                sh.tick(&mut ds, self.frame_count);
            }
        }

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

        // --- Save busmap sidecar if requested (map_bus_window persists) ---
        let needs_busmap_save = self.debug_state.try_lock()
            .map(|mut ds| { let v = ds.save_busmap; ds.save_busmap = false; v })
            .unwrap_or(false);
        if needs_busmap_save {
            if let Some(path) = self.busmap_path.clone() {
                save_busmap_sidecar(&path, &self.debug_state);
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

        // --- Signal frame completion (MCP `step`/`run_frames` synchrony) ---
        // Bumped exactly here, after every bit of this frame's work above —
        // bus-window refresh, state/shadow/record drains, pins, training and
        // shadow ticks, bookmarks, sidecar saves — has already happened. This
        // is the ONLY point that counts as "the frame finished"; a waiter
        // that wakes up here is safe to change input or read state next
        // (docs/frames.md §3 precondition 6 ("let the frame finish") — this replaces the old poll-frame_count-then-
        // sleep-8ms settle with an actual completion signal).
        {
            let mut ds = self.debug_state.lock().unwrap();
            ds.step_generation = ds.step_generation.wrapping_add(1);
            ds.frame_cv.notify_all();
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
    /// Controller port 1 (P2) button states — separate parallel array so the
    /// existing port-0 path and its tests are untouched.
    pub input_state2: [bool; 12],
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
            input_state2: [false; 12],
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
                        // retro_log_printf_t is C-variadic; stable Rust cannot
                        // define one, so the entry point lives in src/log_shim.c
                        // (vsnprintf into a fixed buffer) and calls back into
                        // crate::core_log::rr_core_log_sink for prefixing +
                        // rate limiting.
                        (*(data as *mut RetroLogCallback)).log =
                            crate::core_log::rr_core_log as *const std::ffi::c_void;
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
                | RETRO_ENVIRONMENT_SET_ROTATION
                | RETRO_ENVIRONMENT_SET_GEOMETRY
                | RETRO_ENVIRONMENT_SET_SUPPORT_NO_GAME
                | RETRO_ENVIRONMENT_SET_SUBSYSTEM_INFO
                | RETRO_ENVIRONMENT_SET_CONTROLLER_INFO
                | RETRO_ENVIRONMENT_SET_SERIALIZATION_QUIRKS => true,
                RETRO_ENVIRONMENT_SET_INPUT_DESCRIPTORS => {
                    if !data.is_null() {
                        self.handle_set_input_descriptors(
                            data as *const crate::libretro::RetroInputDescriptor,
                        );
                    }
                    true
                }
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

    /// Capture the core's per-game button names (layer-3 vocabulary — e.g.
    /// FBNeo sends "Weak attack" / "Jab Punch" per RETRO id per game). Stored
    /// in DebugState for the action-vocabulary resolver; joypad ids 0-11 on
    /// ports 0/1 only. The array is NULL-description-terminated.
    fn handle_set_input_descriptors(
        &mut self,
        descs: *const crate::libretro::RetroInputDescriptor,
    ) {
        const RETRO_DEVICE_JOYPAD: u32 = 1;
        let mut captured: Vec<(usize, usize, String)> = Vec::new();
        unsafe {
            let mut p = descs;
            // Hard cap: a missing terminator must not walk off the end.
            for _ in 0..256 {
                if p.is_null() || (*p).description.is_null() {
                    break;
                }
                let d = *p;
                if d.device & 0xFF == RETRO_DEVICE_JOYPAD
                    && d.port < 2
                    && d.id < 12
                {
                    let label = std::ffi::CStr::from_ptr(d.description)
                        .to_string_lossy()
                        .into_owned();
                    captured.push((d.port as usize, d.id as usize, label));
                }
                p = p.add(1);
            }
        }
        if let Ok(mut ds) = self.debug_state.lock() {
            // A fresh SET replaces the previous vocabulary wholesale.
            ds.input_descriptors = Default::default();
            let n = captured.len();
            for (port, id, label) in captured {
                ds.input_descriptors[port][id] = Some(label);
            }
            if n > 0 {
                eprintln!("[input] core provided {n} input descriptors");
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
        if device == RETRO_DEVICE_JOYPAD && (id as usize) < 12 {
            match port {
                0 => self.input_state[id as usize] as i16,
                1 => self.input_state2[id as usize] as i16,
                _ => 0,
            }
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

/// Busmap sidecar file format — the bus windows for one ROM (Sek bridge).
#[derive(serde::Serialize, serde::Deserialize)]
struct BusmapSidecar {
    windows: Vec<crate::debug::BusWindowCfg>,
}

/// Load a busmap sidecar and install its windows. Silently ignores a missing
/// file; parse errors are reported (a hand-authored file deserves a message).
fn load_busmap_sidecar(path: &std::path::Path, debug_state: &SharedDebugState) {
    let data = match std::fs::read_to_string(path) {
        Ok(d) => d,
        Err(_) => return,
    };
    match serde_json::from_str::<BusmapSidecar>(&data) {
        Ok(sidecar) => {
            if let Ok(mut ds) = debug_state.lock() {
                let mut installed = 0usize;
                for cfg in sidecar.windows {
                    if ds.install_bus_window(cfg) {
                        installed += 1;
                    }
                }
                ds.log(format!(
                    "📂 Installed {installed} bus window(s) from {}",
                    path.display()
                ));
            }
        }
        Err(e) => eprintln!("[bus] Failed to parse busmap {}: {}", path.display(), e),
    }
}

/// Save the current bus windows to the busmap sidecar (atomic write via .tmp).
fn save_busmap_sidecar(path: &std::path::Path, debug_state: &SharedDebugState) {
    let windows = match debug_state.lock() {
        Ok(ds) => ds.bus_windows.clone(),
        Err(_) => return,
    };
    let json = match serde_json::to_string_pretty(&BusmapSidecar { windows }) {
        Ok(j) => j,
        Err(e) => {
            eprintln!("[bus] busmap serialization failed: {e}");
            return;
        }
    };
    let tmp = path.with_extension("busmap.json.tmp");
    if std::fs::write(&tmp, &json).is_ok() {
        if let Err(e) = std::fs::rename(&tmp, path) {
            eprintln!("[bus] Failed to rename busmap sidecar: {e}");
        } else if let Ok(mut ds) = debug_state.lock() {
            ds.log(format!("💾 Saved busmap to {}", path.display()));
        }
    } else {
        eprintln!("[bus] Failed to write busmap to {}", tmp.display());
    }
}

/// `<name>.meta.json` sidecar written next to every state saved to an
/// `arenas/` path (see [`Frontend::write_arena_sidecar`]) — the mission this
/// module implements: nobody should burn a verification cycle on a save
/// state whose dummy cannot be driven, ever again. Follows the recorder's
/// sidecar CONVENTION (`record.rs`'s `.meta.json`: same extension swap via
/// `Path::with_extension`, same "everything the reader needs without the
/// live profile" spirit) without sharing its code.
///
/// Arenas saved before this feature existed simply have no sidecar —
/// [`load_arena_meta`] returns `None` for those, and callers render that as
/// "unknown", never as a fabricated answer.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq)]
pub struct ArenaMeta {
    /// Schema tag, mirroring the recorder's `"format": "jsonl-v3"` field.
    pub format: String,
    pub family: String,
    pub port: String,
    /// Canonical roster ids (`GameProfile::canon_char_id`) for each fighter
    /// block, `None` if the block's `char_id` wasn't readable at save time.
    pub char_id_block1: Option<u8>,
    pub char_id_block2: Option<u8>,
    pub health_block1: Option<u8>,
    pub health_block2: Option<u8>,
    /// The controllable gate, evaluated at save time.
    pub gate_open: bool,
    pub inputs_live: InputsLive,
    /// RFC 3339 timestamp (UTC), matching the recorder's `"created"`.
    pub saved_at: String,
}

/// Per-controller-port empirical drivability (mission task 2). `None` means
/// "unknown, honestly" — see [`liveness_precheck`] for the three reasons a
/// port never gets probed.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InputsLive {
    pub p0: Option<bool>,
    pub p1: Option<bool>,
}

/// Whether a just-drained state op should force `paused = true`, per
/// `docs/frames.md` §4.6's atomic load-and-pause fix. Factored out as a pure
/// function (no lock, no core) so the decision itself is unit-testable
/// without a real `Frontend`/core — `drain_state_op` is the only caller, and
/// applies the result inside the SAME lock acquisition that publishes
/// `state_op_result`.
///
/// Only a LOAD that actually SUCCEEDED forces the pause: a failed load
/// leaves the previous (unloaded) machine state running exactly as before,
/// so forcing a pause on failure would be a surprising side effect for a
/// call that didn't change anything; a save is never state that needs
/// "landing" from the caller's point of view. `pause_after: false` (the
/// default — existing `resume → load → poll → pause` callers never set it)
/// always returns `false`, so old callers see byte-for-byte the same
/// behavior as before this fix: `paused` is untouched by the drain.
///
/// This is deliberately simpler than `run_frames`' `fold_generation`/condvar
/// wait: that pattern exists to prove a SEPARATE thread's fold observed a
/// mutation before a gate opens. Here there is only one emulation thread,
/// and `run_frame` is only ever re-entered by that same thread's own next
/// loop iteration — so setting `paused` before this function returns (still
/// holding the lock `run_frame` will next acquire) is already
/// happens-before the very next `run_frame` call's pause check, by plain
/// single-thread program order. No cross-thread visibility race to close.
fn state_op_forces_pause(pause_after: bool, is_load: bool, succeeded: bool) -> bool {
    pause_after && is_load && succeeded
}

/// Whether `path` lives under an `arenas` directory component — the signal
/// that gates the sidecar + liveness probe. Matches the `shadow/arenas/
/// <family>/` convention the Arena panel writes to; a caller that routes a
/// verification copy through any other `.../arenas/...` path (e.g. a temp
/// dir) gets the same treatment. Deliberately NOT keyed off `StateOp`
/// variant alone — `StateOp::Save` is also how the generic State panel and
/// MCP `save_state --path` name an arbitrary file, and only slot saves
/// (`StateOp::SaveSlot`, F6/quick-save) are unconditionally excluded by the
/// caller before this is even consulted.
fn is_arena_path(path: &Path) -> bool {
    path.components().any(|c| c.as_os_str() == "arenas")
}

/// The three honest-degradation reasons from mission task 2, factored out as
/// a pure function so they're testable without a live core: a recording in
/// progress, a closed gate, or a profile that can't resolve `x` for EITHER
/// block, by any means (see [`x_is_readable`] — task B-fix extended this
/// from "has a fixed address" to "fixed OR pointer-resolved"). Returns
/// `Some(InputsLive::default())` (both `None`) when the probe must be
/// skipped, or `None` when it's safe to proceed — the caller then probes
/// whichever of `x1_mapped`/`x2_mapped` individually resolved.
fn liveness_precheck(
    recording_active: bool,
    gate_open: bool,
    x1_mapped: bool,
    x2_mapped: bool,
) -> Option<InputsLive> {
    if recording_active || !gate_open || (!x1_mapped && !x2_mapped) {
        return Some(InputsLive::default());
    }
    None
}

/// Whether fighter field `x` is resolvable for `block` (1 or 2) under the
/// loaded profile — statically addressed (`field_addr`) OR pointer-resolved
/// (`via: "object_ptr"`, docs/frames.md §5). Task B-fix: before this, the
/// arena probe asked `field_addr` alone, which correctly returns `None` for
/// an object-pointer field (there IS no fixed address) but made the whole
/// probe look permanently unmapped on MK2 arcade — `liveness_precheck` then
/// always skipped to `{p0:null,p1:null}`, honest but useless. A pointer-
/// resolved field can still fail to resolve on any GIVEN frame (that's
/// [`judge_burst_motion_differential`]'s job to tell apart from "no
/// motion"), but it is readable in the sense this precheck cares about.
fn x_is_readable(profile: &crate::profile::GameProfile, block: u8) -> bool {
    profile.field_is_object_ptr("x") || profile.field_addr(block, "x").is_some()
}

/// Endian-correct a raw multi-byte read of `size` bytes (1/2/4) for the
/// `via: "object_ptr"` live-read path (docs/frames.md §5) — the pointer word
/// is 4 bytes, the resolved field 1 or 2. Mirrors `training.rs`'s and
/// `record.rs`'s private `endian_fix` (duplicated rather than shared, same
/// rationale as those files' existing `rd8`/`rd16` duplication).
fn endian_fix(v: u32, size: u8, little: bool) -> u32 {
    if little {
        return v;
    }
    match size {
        2 => (v as u16).swap_bytes() as u32,
        4 => v.swap_bytes(),
        _ => v,
    }
}

/// Per-burst-direction, DIFFERENTIAL motion judgement (docs/frames.md §4.2's
/// mandatory differential form, applied to the arena probe): `probe` and
/// `control` are each one `(going_right, x)` per emulated frame, in the SAME
/// order and burst cadence `run_liveness_window` drove from an IDENTICAL
/// starting snapshot — `control` with no input on either port, `probe` with
/// the alternating hold. Direction alternates every `burst_frames` samples.
///
/// Within EACH run, a burst's running delta is `latest − its own first
/// resolved sample`, sign-flipped for a leftward burst, so a positive value
/// always means "moved the way it was told to" — this is what makes MK2's
/// measured ASYMMETRIC walk speeds (+12px/6f forward vs +5px/6f backward) a
/// non-issue; a "hold, undo, expect to return to start" design would wash
/// out the slower direction, but judging each burst's own start→latest delta
/// never does.
///
/// A burst counts as LIVE only when the probe run's delta exceeds the
/// control run's delta AT THE SAME instant by `threshold` — never the
/// probe's raw delta alone. This is what tells a genuinely human-driven port
/// apart from a CPU-driven one: a CPU-controlled block can move on its own
/// initiative during the window (live-verified against
/// `shadow/arenas/mk2/reptile-vs-reptile.state`, a documented 1P-vs-CPU rig
/// — its CPU-driven port satisfied a raw/absolute delta test regardless of
/// injected input), and since a deterministic core replays that same
/// autonomous path identically whether or not the OTHER run holds a
/// direction, the control run cancels it out exactly the way a differential
/// probe cancels pushback and hitstop (docs/frames.md §1.2/§4.2).
///
/// Returns `None` — honestly unknown, never a guessed `false` — when not a
/// single sample resolved in the PROBE run (e.g. an object-pointer port
/// whose pointer never validated during the whole window). A control run
/// that never resolves a given burst is treated as "no observed autonomous
/// motion" (delta 0) for that burst — the probe's own delta then decides it,
/// same as the non-differential case.
fn judge_burst_motion_differential(
    probe: &[(bool, Option<i64>)],
    control: &[(bool, Option<i64>)],
    burst_frames: usize,
    threshold: i64,
) -> Option<bool> {
    let burst_frames = burst_frames.max(1);
    let mut any_probe_sample = false;
    let mut any_live = false;
    let mut cur_burst: Option<usize> = None;
    let mut probe_base: Option<i64> = None;
    let mut control_base: Option<i64> = None;
    let n = probe.len().min(control.len());
    for i in 0..n {
        let (going_right, px) = probe[i];
        let (_, cx) = control[i];
        let burst = i / burst_frames;
        if Some(burst) != cur_burst {
            cur_burst = Some(burst);
            probe_base = None;
            control_base = None;
        }
        let probe_delta = match px {
            None => None,
            Some(v) => {
                any_probe_sample = true;
                match probe_base {
                    None => {
                        probe_base = Some(v);
                        None
                    }
                    Some(base) => Some(if going_right { v - base } else { base - v }),
                }
            }
        };
        let control_delta = match cx {
            None => None,
            Some(v) => match control_base {
                None => {
                    control_base = Some(v);
                    None
                }
                Some(base) => Some(if going_right { v - base } else { base - v }),
            },
        };
        if let Some(pd) = probe_delta {
            if pd - control_delta.unwrap_or(0) >= threshold {
                any_live = true;
            }
        }
    }
    any_probe_sample.then_some(any_live)
}

/// Read `<state>.meta.json` for a saved arena, if one exists. Returns `None`
/// for a missing file (no sidecar — an arena saved before this feature, or
/// simply not under `arenas/`) or a corrupt one; callers render both as
/// "unknown", never as a fabricated answer (per-file house rule).
pub fn load_arena_meta(state_path: &Path) -> Option<ArenaMeta> {
    let meta_path = state_path.with_extension("meta.json");
    let data = std::fs::read_to_string(&meta_path).ok()?;
    serde_json::from_str(&data).ok()
}

/// Compute the SHA-1 of `data` and return it as a lowercase hex string.
///
/// A small self-contained implementation (RFC 3174) so we can stamp the ROM-map
/// frontmatter identity key (§3 of `ROM_MAP_FORMAT.md`) without pulling in a new
/// crate dependency. ROMs are read once at load, so the cost is negligible.
fn sha1_hex(data: &[u8]) -> String {
    let mut h: [u32; 5] = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0];
    let ml = (data.len() as u64).wrapping_mul(8);

    // Build the padded message: data || 0x80 || 0x00... || 64-bit big-endian length.
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&ml.to_be_bytes());

    let mut w = [0u32; 80];
    for chunk in msg.chunks_exact(64) {
        for (i, word) in w.iter_mut().take(16).enumerate() {
            *word = u32::from_be_bytes([
                chunk[i * 4],
                chunk[i * 4 + 1],
                chunk[i * 4 + 2],
                chunk[i * 4 + 3],
            ]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }

        let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
        for (i, &word) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A827999u32),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
                _ => (b ^ c ^ d, 0xCA62C1D6),
            };
            let tmp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = tmp;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }

    let mut out = String::with_capacity(40);
    for word in h {
        out.push_str(&format!("{word:08x}"));
    }
    out
}

#[cfg(test)]
mod state_tests {
    use super::{state_op_forces_pause, state_slot_path};
    use crate::debug::{DebugState, StateOp, StateOpDone};
    use std::path::{Path, PathBuf};

    // ── atomic load-and-pause (docs/frames.md §4.6) ─────────────────────────

    #[test]
    fn state_op_forces_pause_only_on_a_successful_load_with_pause_after_requested() {
        // The one case that must force a pause.
        assert!(state_op_forces_pause(true, true, true));

        // Everything else must NOT — each flips exactly one input off `true`.
        assert!(!state_op_forces_pause(false, true, true), "pause_after not requested");
        assert!(!state_op_forces_pause(true, false, true), "a save, not a load");
        assert!(!state_op_forces_pause(true, true, false), "the load FAILED");
        assert!(!state_op_forces_pause(false, false, false));
    }

    #[test]
    fn state_op_forces_pause_default_false_matches_pre_fix_behavior() {
        // Existing `resume -> load -> poll -> pause` callers never set
        // pause_after, so the drain must never touch `paused` for them —
        // byte-for-byte the same as before this fix existed.
        for is_load in [true, false] {
            for succeeded in [true, false] {
                assert!(!state_op_forces_pause(false, is_load, succeeded));
            }
        }
    }

    /// The DebugState-level handoff, extended from `state_op_queue_handoff`
    /// below: `pending_state_op_pause_after` travels alongside
    /// `pending_state_op` (queued together, taken together by
    /// `drain_state_op`'s single `try_lock`), and a successful load with it
    /// set forces `paused = true` in the SAME critical section that
    /// publishes `state_op_result` — mirroring `drain_state_op` exactly, but
    /// without needing a real core.
    #[test]
    fn pause_after_travels_with_the_op_and_is_applied_atomically_with_the_result() {
        let mut ds = DebugState::new();
        assert!(!ds.paused);
        assert!(!ds.pending_state_op_pause_after);

        // Requesting side (MCP load_state{pause_after: true}) queues both
        // fields under one lock.
        ds.pending_state_op = Some(StateOp::LoadSlot(1));
        ds.pending_state_op_pause_after = true;

        // Emu-thread drain takes both together (drain_state_op's contract).
        let op = ds.pending_state_op.take();
        let pause_after = std::mem::take(&mut ds.pending_state_op_pause_after);
        assert_eq!(op, Some(StateOp::LoadSlot(1)));
        assert!(pause_after);
        assert!(!ds.pending_state_op_pause_after, "consumed, not left dangling for the next op");

        // Drain "succeeds" (stand-in for a real do_load_state Ok) and, in
        // the SAME lock acquisition, forces the pause before publishing the
        // result — exactly what a caller polling `state_op_result` needs to
        // never observe one without the other.
        let done = StateOpDone { loaded: true, path: PathBuf::from("/saves/mk2.state1"), bytes: 4096 };
        if state_op_forces_pause(pause_after, op.as_ref().map_or(false, |o| matches!(o, StateOp::Load(_) | StateOp::LoadSlot(_))), true) {
            ds.paused = true;
        }
        ds.state_op_result = Some(Ok(done.clone()));

        assert!(ds.paused, "pause_after load must leave the emulator paused");
        assert_eq!(ds.state_op_result.take(), Some(Ok(done)));
    }

    #[test]
    fn pause_after_is_not_sticky_across_a_later_plain_op() {
        // Regression for the "always consumed" half of the contract: a
        // pause_after request must not leak onto a later op that never
        // asked for it.
        let mut ds = DebugState::new();
        ds.pending_state_op = Some(StateOp::LoadSlot(1));
        ds.pending_state_op_pause_after = true;
        let _ = ds.pending_state_op.take();
        let _ = std::mem::take(&mut ds.pending_state_op_pause_after);

        // A second, later op queued WITHOUT pause_after...
        ds.pending_state_op = Some(StateOp::LoadSlot(2));
        let pause_after = std::mem::take(&mut ds.pending_state_op_pause_after);
        assert!(!pause_after, "must not inherit the previous request");
    }

    #[test]
    fn slot_path_follows_sidecar_convention() {
        assert_eq!(
            state_slot_path(Path::new("/saves"), "asurabld", 1),
            PathBuf::from("/saves/asurabld.state1")
        );
        assert_eq!(
            state_slot_path(Path::new("."), "mvsc", 9),
            PathBuf::from("./mvsc.state9")
        );
    }

    /// The UI/MCP → emu-thread handoff at DebugState level: queue an op, the
    /// drain side takes it (queue is now empty — no double-execution), and the
    /// published result is visible to (and cleared by) the requesting side.
    #[test]
    fn state_op_queue_handoff() {
        let mut ds = DebugState::new();
        assert!(ds.pending_state_op.is_none());

        // Requesting side queues.
        ds.pending_state_op = Some(StateOp::SaveSlot(2));

        // Emu-thread drain takes exactly once.
        let op = ds.pending_state_op.take();
        assert_eq!(op, Some(StateOp::SaveSlot(2)));
        assert!(ds.pending_state_op.is_none());

        // Drain publishes the completion record.
        let done = StateOpDone {
            loaded: false,
            path: PathBuf::from("/saves/asurabld.state2"),
            bytes: 1234,
        };
        ds.state_op_result = Some(Ok(done.clone()));

        // Requesting side polls and clears.
        let got = ds.state_op_result.take();
        assert_eq!(got, Some(Ok(done)));
        assert!(ds.state_op_result.is_none());

        // Explicit-path ops round-trip too.
        ds.pending_state_op = Some(StateOp::Load(PathBuf::from("/tmp/x.state")));
        assert_eq!(
            ds.pending_state_op.take(),
            Some(StateOp::Load(PathBuf::from("/tmp/x.state")))
        );
    }
}

#[cfg(test)]
mod sha1_tests {
    use super::sha1_hex;

    #[test]
    fn sha1_known_vectors() {
        assert_eq!(sha1_hex(b""), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        assert_eq!(sha1_hex(b"abc"), "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert_eq!(
            sha1_hex(b"The quick brown fox jumps over the lazy dog"),
            "2fd4e1c67a2d28fced849ee1bb76e7391b93eb12"
        );
    }
}

#[cfg(test)]
mod arena_sidecar_tests {
    use super::{
        is_arena_path, judge_burst_motion_differential, liveness_precheck, load_arena_meta, x_is_readable,
        ArenaMeta, InputsLive,
    };
    use std::path::Path;

    #[test]
    fn arena_meta_serializes_with_the_expected_shape() {
        let meta = ArenaMeta {
            format: "arena-meta-v1".into(),
            family: "mk2".into(),
            port: "arcade".into(),
            char_id_block1: Some(9),
            char_id_block2: None,
            health_block1: Some(161),
            health_block2: None,
            gate_open: true,
            inputs_live: InputsLive { p0: Some(true), p1: Some(false) },
            saved_at: "2026-08-28T00:00:00Z".into(),
        };
        let json = serde_json::to_value(&meta).unwrap();
        assert_eq!(json["format"], "arena-meta-v1");
        assert_eq!(json["family"], "mk2");
        assert_eq!(json["port"], "arcade");
        assert_eq!(json["char_id_block1"], 9);
        assert!(json["char_id_block2"].is_null());
        assert_eq!(json["gate_open"], true);
        assert_eq!(json["inputs_live"]["p0"], true);
        assert_eq!(json["inputs_live"]["p1"], false);

        // Round-trips losslessly — the panel reads this back via `load_arena_meta`.
        let back: ArenaMeta = serde_json::from_value(json).unwrap();
        assert_eq!(back, meta);
    }

    #[test]
    fn load_arena_meta_is_none_for_a_missing_sidecar() {
        // Every arena committed before this feature has no `.meta.json` —
        // this must render as "unknown", never panic or fabricate a value.
        assert_eq!(
            load_arena_meta(Path::new("/tmp/rustretro_definitely_missing_arena.state")),
            None
        );
    }

    #[test]
    fn load_arena_meta_is_none_for_a_corrupt_sidecar() {
        let dir = std::env::temp_dir().join(format!(
            "rustretro_arena_meta_test_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let state_path = dir.join("bad.state");
        std::fs::write(state_path.with_extension("meta.json"), b"not json").unwrap();
        assert_eq!(load_arena_meta(&state_path), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn is_arena_path_matches_the_arenas_directory_convention() {
        assert!(is_arena_path(Path::new("shadow/arenas/mk2/r-v-r.state")));
        assert!(is_arena_path(Path::new("/tmp/jobs/verify/arenas/copy.state")));
        assert!(!is_arena_path(Path::new("saves/mk2.state1")));
        assert!(!is_arena_path(Path::new("shadow/recordings/mk2/session.state")));
    }

    /// The three honest-degradation reasons (mission task 2): a recording in
    /// progress, a closed gate, or no `x` mapped for either block all must
    /// skip the probe and report BOTH ports unknown — never a guess.
    #[test]
    fn liveness_precheck_degrades_to_null_when_recording_active() {
        assert_eq!(liveness_precheck(true, true, true, true), Some(InputsLive::default()));
    }

    #[test]
    fn liveness_precheck_degrades_to_null_when_gate_closed() {
        assert_eq!(liveness_precheck(false, false, true, true), Some(InputsLive::default()));
    }

    #[test]
    fn liveness_precheck_degrades_to_null_when_no_x_mapped_for_either_block() {
        assert_eq!(liveness_precheck(false, true, false, false), Some(InputsLive::default()));
    }

    #[test]
    fn liveness_precheck_proceeds_when_open_recorder_idle_and_one_block_maps_x() {
        // Only block1 maps `x` (e.g. a port that hasn't mapped block2 yet) —
        // the precheck still says "proceed"; the per-port `None` for the
        // unmapped block happens downstream.
        assert_eq!(liveness_precheck(false, true, true, false), None);
    }

    /// Task B-fix (docs/frames.md §5): `x` mapped via `via: "object_ptr"`
    /// (no fixed address at all) must still count as "readable" for the
    /// precheck — this is the actual fix for MK2 arcade's arena probe
    /// having gone from wrong (`false`/`false`) to useless (`null`/`null`)
    /// when `x` moved to the pointer-resolved schema.
    #[test]
    fn x_is_readable_true_for_object_ptr_field_with_no_fixed_address() {
        let p = crate::profile::GameProfile::load(Path::new("library/mk2")).expect("mk2 profile loads");
        assert!(p.field_is_object_ptr("x"), "mk2 arcade's x is via: object_ptr");
        assert_eq!(p.field_addr(1, "x"), None, "…and therefore has no fixed address");
        assert!(x_is_readable(&p, 1), "must still count as readable");
        assert!(x_is_readable(&p, 2));
    }

    #[test]
    fn x_is_readable_true_for_a_plain_off_field() {
        let p = crate::profile::init_for_tests(); // asurabld: x is a plain `off` field
        assert!(x_is_readable(&p, 1));
        assert!(x_is_readable(&p, 2));
    }

    #[test]
    fn x_is_readable_false_when_the_profile_maps_no_x_at_all() {
        let p = crate::profile::GameProfile::load(Path::new("library/sf2ce")).expect("sf2ce profile loads");
        assert!(!x_is_readable(&p, 1));
        assert!(!x_is_readable(&p, 2));
    }

    /// A frozen control (no autonomous motion) means the probe's OWN raw
    /// delta decides it — this is the differential judge degenerating to the
    /// simple case, and it must detect BOTH directions independently even
    /// though MK2's measured walk speeds are asymmetric (+12px/6f forward vs
    /// +5px/6f backward): a design shaped as "hold, undo, expect to return
    /// to start" would wash the slower direction out; judging each burst's
    /// own start→latest delta never does.
    #[test]
    fn judge_burst_motion_differential_detects_both_asymmetric_directions_over_a_still_control() {
        // Burst 0 (right, 6 frames): +10 over the burst (measured forward rate).
        let mut probe: Vec<(bool, Option<i64>)> = (0..6).map(|i| (true, Some(100 + i * 2))).collect();
        // Burst 1 (left, 6 frames): +5 over the burst (measured backward rate) —
        // position DECREASES in world terms, x values fall.
        probe.extend((0..6).map(|i| (false, Some(110 - i))));
        let control: Vec<(bool, Option<i64>)> = probe.iter().map(|(r, _)| (*r, Some(0))).collect();
        assert_eq!(judge_burst_motion_differential(&probe, &control, 6, 4), Some(true));
    }

    #[test]
    fn judge_burst_motion_differential_asymmetric_backward_alone_is_not_washed_out() {
        // Only the SLOWER (backward) direction moves; forward is frozen.
        // A "return to start" style check would see net-zero across the
        // pair and call it dead; the per-burst differential judge must not.
        let mut probe: Vec<(bool, Option<i64>)> = (0..6).map(|_| (true, Some(200))).collect(); // frozen
        probe.extend((0..6).map(|i| (false, Some(200 - i)))); // −5 over the burst
        let control: Vec<(bool, Option<i64>)> = probe.iter().map(|(r, _)| (*r, Some(0))).collect();
        assert_eq!(judge_burst_motion_differential(&probe, &control, 6, 4), Some(true));
    }

    #[test]
    fn judge_burst_motion_differential_false_when_frozen_every_burst() {
        let mut probe: Vec<(bool, Option<i64>)> = (0..6).map(|_| (true, Some(50))).collect();
        probe.extend((0..6).map(|_| (false, Some(50))));
        let control: Vec<(bool, Option<i64>)> = probe.iter().map(|(r, _)| (*r, Some(0))).collect();
        assert_eq!(judge_burst_motion_differential(&probe, &control, 6, 4), Some(false));
    }

    /// The headline fix, live-verified against `shadow/arenas/mk2/
    /// reptile-vs-reptile.state` (a documented 1P-vs-CPU rig): a CPU-driven
    /// block moves on its OWN initiative regardless of injected input — the
    /// control run (no input on either port) reproduces that SAME
    /// autonomous path, so the excess over control is ~0 and the port must
    /// NOT be reported live, even though its raw/absolute delta alone would
    /// clear the threshold (exactly what the old absolute design got wrong).
    #[test]
    fn judge_burst_motion_differential_rejects_autonomous_cpu_motion_matched_in_control() {
        let mut probe: Vec<(bool, Option<i64>)> = (0..6).map(|i| (true, Some(300 + i * 2))).collect();
        probe.extend((0..6).map(|i| (false, Some(310 - i))));
        // The CPU walks the SAME path whether or not a human holds a
        // direction on its port — deterministic replay from an identical
        // snapshot reproduces it exactly.
        let control = probe.clone();
        assert_eq!(
            judge_burst_motion_differential(&probe, &control, 6, 4),
            Some(false),
            "CPU's own motion must not be mistaken for injected-input liveness"
        );
    }

    #[test]
    fn judge_burst_motion_differential_none_when_no_probe_sample_resolves() {
        let probe: Vec<(bool, Option<i64>)> = (0..12).map(|i| (i < 6, None)).collect();
        let control: Vec<(bool, Option<i64>)> = (0..12).map(|i| (i < 6, Some(0))).collect();
        assert_eq!(
            judge_burst_motion_differential(&probe, &control, 6, 4),
            None,
            "honestly unknown, never a guessed false"
        );
    }

    #[test]
    fn judge_burst_motion_differential_tolerates_sparse_object_ptr_gaps_within_a_burst() {
        // A `via: "object_ptr"` port whose pointer only resolves on SOME
        // frames of the burst must still detect the motion that did land,
        // in BOTH the probe and (if sparse there too) the control run.
        let probe: Vec<(bool, Option<i64>)> =
            vec![(true, Some(100)), (true, None), (true, None), (true, Some(112)), (true, None), (true, Some(116))];
        let control: Vec<(bool, Option<i64>)> = (0..6).map(|_| (true, Some(0))).collect();
        assert_eq!(judge_burst_motion_differential(&probe, &control, 6, 4), Some(true));
    }

    #[test]
    fn judge_burst_motion_differential_missing_control_burst_falls_back_to_probes_own_delta() {
        // The control run's pointer never resolves this burst at all — no
        // observed autonomous motion to subtract, so the probe's own delta
        // decides it (never treated as "unknown" just because control was
        // sparse — only the PROBE run's absence is honored that way).
        let probe: Vec<(bool, Option<i64>)> = (0..6).map(|i| (true, Some(100 + i * 2))).collect();
        let control: Vec<(bool, Option<i64>)> = (0..6).map(|_| (true, None)).collect();
        assert_eq!(judge_burst_motion_differential(&probe, &control, 6, 4), Some(true));
    }
}
