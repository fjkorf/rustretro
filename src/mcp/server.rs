//! The rmcp [`ServerHandler`] that exposes the live emulator/debugger to a
//! Claude session over the Model Context Protocol.
//!
//! ## Threading
//! This handler holds a CLONE of the `Arc<Mutex<DebugState>>`. It runs entirely
//! on the MCP server thread (its own tokio runtime) and only ever locks the
//! mutex briefly to read/copy data. It NEVER touches the NonSend `Emu`/`Lua`
//! resources — that's the whole reason `DebugState` is the shared boundary.
//!
//! The one exception is `run_lua`, which can't run Lua here (the engine is a
//! main-thread NonSend resource). Instead it writes the script into
//! `DebugState::pending_lua` and polls `DebugState::pending_lua_result`; the
//! Bevy `drain_lua_requests` system on the main thread does the actual work.
//!
//! ## Scope (AI Wave 1)
//! Read-mostly perception plus a small SAFE control set (pause/resume/step) and
//! the gated `run_lua` bridge. Unsafe writes (write_memory, freeze, breakpoint
//! set) are intentionally NOT implemented this wave.

use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rmcp::handler::server::ServerHandler;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, Content, ErrorData, Implementation,
    ListResourcesResult, ListToolsResult, PaginatedRequestParams, ProtocolVersion,
    RawResource, ReadResourceRequestParams, ReadResourceResult, ResourceContents,
    ServerCapabilities, ServerInfo, Tool,
};
use rmcp::service::RequestContext;
use rmcp::RoleServer;
use serde_json::{json, Map, Value};

use crate::debug::{SharedDebugState, StateOp, Watch, WatchFormat};
use crate::mcp::ines::parse_ines;
use crate::mcp::snapshot::{
    decode_tiles_to_rgba, memory_capability, memory_map, parse_hex_bytes, read_region_bytes,
    rgba_to_png, scan_buffer, search_bytes, top_heatmap, AiSnapshot, TileFormat,
};

/// How long the `run_lua` tool waits for the main thread to execute a script
/// before giving up and returning a timeout error.
const LUA_TIMEOUT: Duration = Duration::from_secs(5);
/// How long `save_state`/`load_state` wait for the emulation thread to drain
/// the queued state op (it resolves within a frame or two, like `run_lua`).
const STATE_TIMEOUT: Duration = Duration::from_secs(5);
/// Cap on bytes returned by `read_memory` to avoid huge dumps.
const MAX_READ_LEN: usize = 4096;
/// Cap on bytes returned by `read_region` (larger than read_memory so Claude can
/// pull a meaningful slab of VRAM/object-RAM in one call).
const MAX_REGION_READ_LEN: usize = 8192;
/// Minimum needle length for `search_memory` / `vram_to_rom`. Below this the
/// search is meaningless (every short pattern matches everywhere).
const MIN_NEEDLE_LEN: usize = 4;
/// Maximum needle length for `search_memory` / `vram_to_rom`.
const MAX_NEEDLE_LEN: usize = 256;
/// Cap on the number of match addresses returned by a single search.
const MAX_SEARCH_HITS: usize = 256;
/// Number of hottest PCs returned by `app://heatmap`.
const HEATMAP_TOP_N: usize = 64;
/// Cap on bytes decoded by `render_tiles`, so the emitted PNG stays small enough
/// to embed as a tool-result image (64 KB of ROM ≈ 4096 NES tiles → a 256-wide,
/// 1024-tall grid at 16 tiles/row).
const MAX_RENDER_TILES_LEN: usize = 64 * 1024;
/// Default number of tiles laid out per row by `render_tiles`.
const DEFAULT_TILES_PER_ROW: usize = 16;
/// Hard cap on `tiles_per_row` so a pathological value can't make a 1-pixel-tall
/// mile-wide image.
const MAX_TILES_PER_ROW: usize = 64;
/// Default window size (bytes) for `scan_regions` statistical sampling. 4 KB is
/// large enough for entropy to be meaningful, small enough to localize a region
/// boundary to within a few KB.
const DEFAULT_SCAN_WINDOW: usize = 4 * 1024;
/// Floor on the `scan_regions` window so tiny windows can't make entropy noise.
const MIN_SCAN_WINDOW: usize = 256;
/// Ceiling on the `scan_regions` window so a huge window can't collapse a whole
/// ROM into one coarse verdict.
const MAX_SCAN_WINDOW: usize = 64 * 1024;
/// Cap on total bytes `scan_regions` will analyze in one call, so scanning a
/// multi-MB ROM stays bounded. Larger regions are scanned up to this prefix and
/// the result flags the truncation.
const MAX_SCAN_LEN: usize = 8 * 1024 * 1024;
/// Cap on how many frames a single `press_buttons` call holds a button (~10s at
/// 60fps), so a button can't get stuck on indefinitely.
const MAX_INPUT_HOLD_FRAMES: u32 = 600;
/// Human-facing list of valid `press_buttons` names (for error messages).
const JOYPAD_BUTTON_LIST: &str = "a, b, x, y, l, r, start, select, up, down, left, right";
/// Bounded wait for a synchronous `step` to land. Generous relative to a
/// single frame's real cost (≈16.7ms even capped at 60fps, sub-millisecond
/// uncapped) but finite — a wedged emulation thread must produce a clear
/// timeout error, never an indefinitely hung MCP call.
const STEP_WAIT_TIMEOUT: Duration = Duration::from_secs(2);
/// `run_frames` cap — mirrors `MAX_INPUT_HOLD_FRAMES`'s ~10s-at-60fps budget
/// so one call can't make the server unresponsive for an unbounded time.
const MAX_RUN_FRAMES: u32 = 600;
/// Per-frame slice of `run_frames`' overall wait budget, plus a fixed floor
/// for small counts — same "generous but finite" shape as `STEP_WAIT_TIMEOUT`,
/// scaled up because a batch is many frames' worth of waiting.
const RUN_FRAMES_PER_FRAME_TIMEOUT: Duration = Duration::from_millis(100);
const RUN_FRAMES_TIMEOUT_FLOOR: Duration = Duration::from_secs(2);
/// Bounded wait for the host loop's NEXT input fold after `run_frames` sets
/// `port0`/`port1` masks — same "generous but finite" shape as
/// `STEP_WAIT_TIMEOUT`. The fold runs every host-loop tick regardless of
/// `paused` (see `main.rs`'s headless step (a0) / windowed `read_input`), so
/// under normal operation this resolves in well under a millisecond; a
/// timeout here means the host loop itself is wedged, same failure mode
/// `STEP_WAIT_TIMEOUT` names for `step`.
const FOLD_WAIT_TIMEOUT: Duration = Duration::from_secs(2);
/// Sentinel error prefix emitted by [`RetroMcpServer::clone_region_bytes`] when a
/// region is declared but backed by no readable host memory (a virtual/garbage
/// descriptor). Format: `"<prefix>:<region_name>"`.
const REGION_UNBACKED: &str = "region-unbacked";
/// Maximum number of M68K PC breakpoints, mirroring the UI cap in
/// `debug/panels/disassembly.rs` so an MCP-set breakpoint behaves identically.
const MAX_BREAKPOINTS: usize = 8;

/// The controlled vocabulary of region `kind` values (ROM_MAP_FORMAT §5). The
/// `add_rom_map_region` tool validates against this list so AI-authored regions
/// stay queryable across the library.
const ROM_MAP_KINDS: &[&str] = &[
    // Code
    "game_loop",
    "subroutine",
    "interrupt_handler",
    "sound_driver",
    // Graphics
    "title_screen",
    "background",
    "tilemap",
    "character_sprite",
    "sprite_sheet",
    "palette",
    // Audio
    "music_track",
    "sfx_table",
    // Data
    "level_data",
    "text_table",
    "lookup_table",
];

/// The `confidence` vocabulary (ROM_MAP_FORMAT §4). Default is `likely`.
const ROM_MAP_CONFIDENCES: &[&str] = &["confirmed", "likely", "guess"];

/// Default human-zone stub prose for an AI-authored region when no note is given.
const DEFAULT_REGION_NOTE: &str = "Discovered via MCP RE session.";

/// The MCP server handler. Cloneable (it's just an `Arc` inside) so the
/// streamable-http service factory can mint a fresh handler per session.
///
/// ## Write gate
/// `writes_enabled` is the session-level "writes armed" flag. The streamable-http
/// factory mints a FRESH `RetroMcpServer` per MCP session (see `spawn_mcp_server`),
/// so a field here is the correct home for the gate: it persists across tool calls
/// for the lifetime of one session and is naturally isolated per session, while the
/// shared `DebugState` stays a pure data boundary. It is an `Arc<AtomicBool>` so the
/// `#[derive(Clone)]` (used by `call_tool`/`read_resource`, which clone `self`)
/// shares one flag rather than copying it. Defaults to LOCKED (false).
#[derive(Clone)]
pub struct RetroMcpServer {
    debug: SharedDebugState,
    writes_enabled: Arc<AtomicBool>,
}

impl RetroMcpServer {
    pub fn new(debug: SharedDebugState) -> Self {
        RetroMcpServer {
            debug,
            writes_enabled: Arc::new(AtomicBool::new(false)),
        }
    }

    // ── write gate ─────────────────────────────────────────────────────────

    /// Returns `Ok(())` when write tools are armed, `Err` with a refusal message
    /// otherwise. Factored out so the gate logic is unit-testable without a live
    /// MCP server (see tests).
    fn check_writes_armed(&self) -> Result<(), &'static str> {
        if self.writes_enabled.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err("writes are locked; call enable_writes first")
        }
    }

    /// Arm the write tools for this session. Also arms the app-wide Lua
    /// `memory.writebyte`/`memory.writeword` gate ([`DebugState::lua_writes_enabled`])
    /// — one switch controls "may this app poke guest RAM", whichever door
    /// (MCP write_memory or a Lua script) the poke comes through.
    fn enable_writes(&self) -> Value {
        self.writes_enabled.store(true, Ordering::SeqCst);
        if let Ok(mut ds) = self.debug.lock() {
            ds.lua_writes_enabled = true;
        }
        json!({
            "ok": true,
            "writes_enabled": true,
            "message": "Write tools ARMED. write_memory/freeze/set_breakpoint/run_to and the Lua \
                        memory.writebyte/writeword bindings are now active. Call disable_writes \
                        to re-lock.",
        })
    }

    /// Re-lock the write tools for this session (and the Lua write gate; note
    /// this re-locks Lua writes even when `--training` armed them at launch).
    fn disable_writes(&self) -> Value {
        self.writes_enabled.store(false, Ordering::SeqCst);
        if let Ok(mut ds) = self.debug.lock() {
            ds.lua_writes_enabled = false;
        }
        json!({
            "ok": true,
            "writes_enabled": false,
            "message": "Write tools LOCKED (Lua memory writes too).",
        })
    }

    // ── helpers ────────────────────────────────────────────────────────────

    /// Build a JSON `Content` text block from any serializable value.
    fn json_content(v: &impl serde::Serialize) -> Result<Content, ErrorData> {
        let s = serde_json::to_string_pretty(v)
            .map_err(|e| ErrorData::internal_error(format!("serialize error: {e}"), None))?;
        Ok(Content::text(s))
    }

    /// Read `len` bytes starting at guest `addr`, returning a hex string and the
    /// containing region name. Caps `len` at [`MAX_READ_LEN`].
    fn read_memory(&self, addr: usize, len: usize) -> Value {
        let len = len.min(MAX_READ_LEN);
        let ds = match self.debug.lock() {
            Ok(g) => g,
            Err(_) => return json!({ "error": "debug state lock poisoned" }),
        };
        // Find the containing region (for the name) and read byte-by-byte via
        // read_addr (which reads up to 4 bytes per call).
        let region_name = ds
            .memory_regions
            .iter()
            .find(|r| addr >= r.addr_start && addr <= r.addr_end)
            .map(|r| r.name.clone());

        let mut bytes = Vec::with_capacity(len);
        let mut ok = true;
        for i in 0..len {
            match ds.read_addr(addr + i, 1) {
                Some(b) => bytes.push(b as u8),
                None => {
                    ok = false;
                    break;
                }
            }
        }
        drop(ds);

        if region_name.is_none() && bytes.is_empty() {
            return json!({
                "addr": format!("0x{addr:X}"),
                "error": "address not within any mapped region",
            });
        }

        let hex = bytes
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect::<Vec<_>>()
            .join(" ");
        json!({
            "addr": format!("0x{addr:X}"),
            "len": bytes.len(),
            "region": region_name,
            "complete": ok,
            "hex": hex,
        })
    }

    /// Clone the bytes of a named region out from under a brief lock. Returns
    /// `(addr_start, kind, bytes)` or an error string.
    ///
    /// Goes through the bounds-checked [`read_region_bytes`] helper, which uses
    /// `MemoryRegion::safe_host_ptr` and NEVER does a blind
    /// `from_raw_parts(region.ptr, ..)`. A virtual/unbacked descriptor (null or
    /// garbage `ptr`, like NES NTARAM/OAM at 0x8000xxxx) yields a dedicated
    /// [`REGION_UNBACKED`] error instead of crashing, so callers can surface an
    /// honest "declared but not readable" result.
    ///
    /// We materialize the bytes into an owned `Vec` so the caller can drop the
    /// mutex before doing any expensive scanning.
    fn clone_region_bytes(&self, region_name: &str) -> Result<(usize, String, Vec<u8>), String> {
        let ds = self.debug.lock().map_err(|_| "debug state lock poisoned".to_string())?;
        let region = ds
            .memory_regions
            .iter()
            .find(|r| r.name == region_name)
            .ok_or_else(|| format!("no region named '{region_name}'"))?
            .clone();
        drop(ds);
        let kind = region.region_type().to_string();
        let start = region.addr_start;
        // Read the whole region. None == unbacked/virtual descriptor.
        let bytes = read_region_bytes(&region, 0, region.size)
            .ok_or_else(|| format!("{REGION_UNBACKED}:{region_name}"))?;
        Ok((start, kind, bytes))
    }

    /// `read_region`: read `len` bytes from within a NAMED region at `offset`.
    /// Lets Claude inspect VRAM/object-RAM/ROM by name without knowing absolute
    /// guest addresses. Caps `len` at [`MAX_REGION_READ_LEN`].
    fn read_region(&self, region_name: &str, offset: usize, len: usize) -> Value {
        let len = len.min(MAX_REGION_READ_LEN);
        let (start, kind, bytes) = match self.clone_region_bytes(region_name) {
            Ok(t) => t,
            Err(e) if e.starts_with(REGION_UNBACKED) => {
                return json!({
                    "region": region_name,
                    "error": format!(
                        "region '{region_name}' is declared but not backed by readable \
                         memory (virtual descriptor)"
                    ),
                })
            }
            Err(e) => return json!({ "error": e }),
        };
        if offset >= bytes.len() {
            return json!({
                "region": region_name,
                "kind": kind,
                "error": format!(
                    "offset 0x{offset:X} is beyond readable region bytes (len 0x{:X})",
                    bytes.len()
                ),
            });
        }
        let end = (offset + len).min(bytes.len());
        let slice = &bytes[offset..end];
        let hex = slice
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect::<Vec<_>>()
            .join(" ");
        json!({
            "region": region_name,
            "kind": kind,
            "region_addr_start": format!("0x{start:X}"),
            "offset": format!("0x{offset:X}"),
            "abs_addr": format!("0x{:X}", start + offset),
            "len": slice.len(),
            "hex": hex,
        })
    }

    /// `search_memory`: scan one region (or ALL regions when `scope` is "all" or
    /// empty) for the byte pattern `needle`, returning absolute match addresses.
    ///
    /// This is the achievable substitute for true DMA provenance: it is a CONTENT
    /// match, not a transfer trace. Clones each region's bytes under a brief lock,
    /// then scans UNLOCKED via the pure [`search_bytes`] kernel.
    fn search_memory(&self, needle: &[u8], scope: &str) -> Value {
        if needle.len() < MIN_NEEDLE_LEN || needle.len() > MAX_NEEDLE_LEN {
            return json!({
                "error": format!(
                    "needle must be {MIN_NEEDLE_LEN}..={MAX_NEEDLE_LEN} bytes (got {})",
                    needle.len()
                )
            });
        }

        // Snapshot the region list (names + kinds) under a brief lock, then
        // release it before scanning each region.
        let region_names: Vec<(String, String)> = {
            let ds = match self.debug.lock() {
                Ok(g) => g,
                Err(_) => return json!({ "error": "debug state lock poisoned" }),
            };
            ds.memory_regions
                .iter()
                .map(|r| (r.name.clone(), r.region_type().to_string()))
                .collect()
        };

        let all = scope.is_empty() || scope.eq_ignore_ascii_case("all");
        let mut results = Vec::new();
        let mut total_hits = 0usize;
        for (name, _kind) in &region_names {
            if !all && !name.eq_ignore_ascii_case(scope) {
                continue;
            }
            let (start, kind, bytes) = match self.clone_region_bytes(name) {
                Ok(t) => t,
                Err(_) => continue,
            };
            let remaining = MAX_SEARCH_HITS.saturating_sub(total_hits);
            if remaining == 0 {
                break;
            }
            let offsets = search_bytes(&bytes, needle, remaining);
            if offsets.is_empty() {
                continue;
            }
            total_hits += offsets.len();
            let addrs: Vec<String> = offsets
                .iter()
                .map(|o| format!("0x{:X}", start + o))
                .collect();
            results.push(json!({
                "region": name,
                "kind": kind,
                "matches": addrs,
                "match_count": offsets.len(),
            }));
        }

        if !all && !region_names.iter().any(|(n, _)| n.eq_ignore_ascii_case(scope)) {
            return json!({ "error": format!("no region named '{scope}' (use 'all' to scan everything)") });
        }

        json!({
            "needle_len": needle.len(),
            "scope": if all { "all".to_string() } else { scope.to_string() },
            "total_matches": total_hits,
            "capped": total_hits >= MAX_SEARCH_HITS,
            "results": results,
        })
    }

    /// `vram_to_rom`: the headline "where did this tile come from" primitive.
    /// Reads `len` bytes from the VRAM region at absolute `vram_addr`, then
    /// content-searches all ROM-type regions for that exact block.
    ///
    /// HONESTY: this is a CONTENT match, not a DMA-traced provenance. The loaded
    /// cores expose no DMA source→dest hook, so we cannot prove a tile was copied
    /// from a given ROM address — only that identical bytes exist there. Expect
    /// false positives (coincidental matches) and false negatives (when the ROM
    /// stores the graphics compressed or in a different bitplane layout, the raw
    /// VRAM bytes won't appear verbatim).
    fn vram_to_rom(&self, vram_addr: usize, len: usize) -> Value {
        let len = len.clamp(MIN_NEEDLE_LEN, MAX_NEEDLE_LEN);

        // Find the VRAM region containing vram_addr and read `len` bytes from it.
        let (vram_region, needle) = {
            let ds = match self.debug.lock() {
                Ok(g) => g,
                Err(_) => return json!({ "error": "debug state lock poisoned" }),
            };
            let region = ds
                .memory_regions
                .iter()
                .find(|r| vram_addr >= r.addr_start && vram_addr <= r.addr_end)
                .cloned();
            let region = match region {
                Some(r) => r,
                None => {
                    return json!({
                        "error": format!("0x{vram_addr:X} is not within any mapped region"),
                    })
                }
            };
            let mut bytes = Vec::with_capacity(len);
            for i in 0..len {
                match ds.read_addr(vram_addr + i, 1) {
                    Some(b) => bytes.push(b as u8),
                    None => break,
                }
            }
            (region, bytes)
        };

        if needle.len() < MIN_NEEDLE_LEN {
            return json!({
                "error": format!(
                    "could only read {} bytes at 0x{vram_addr:X}; need at least {MIN_NEEDLE_LEN}",
                    needle.len()
                )
            });
        }

        let source_hex = needle
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect::<Vec<_>>()
            .join(" ");

        // Gather ROM-type region names.
        let rom_regions: Vec<String> = {
            let ds = match self.debug.lock() {
                Ok(g) => g,
                Err(_) => return json!({ "error": "debug state lock poisoned" }),
            };
            ds.memory_regions
                .iter()
                .filter(|r| r.region_type() == "ROM")
                .map(|r| r.name.clone())
                .collect()
        };

        let mut candidates = Vec::new();
        let mut total = 0usize;
        for name in &rom_regions {
            let (start, _kind, bytes) = match self.clone_region_bytes(name) {
                Ok(t) => t,
                Err(_) => continue,
            };
            let remaining = MAX_SEARCH_HITS.saturating_sub(total);
            if remaining == 0 {
                break;
            }
            let offsets = search_bytes(&bytes, &needle, remaining);
            if offsets.is_empty() {
                continue;
            }
            total += offsets.len();
            let addrs: Vec<String> =
                offsets.iter().map(|o| format!("0x{:X}", start + o)).collect();
            candidates.push(json!({
                "rom_region": name,
                "candidate_addrs": addrs,
                "match_count": offsets.len(),
            }));
        }

        json!({
            "method": "content-match (NOT DMA-traced provenance)",
            "vram_addr": format!("0x{vram_addr:X}"),
            "vram_region": vram_region.name,
            "source_len": needle.len(),
            "source_hex": source_hex,
            "rom_regions_searched": rom_regions,
            "rom_candidates": candidates,
            "total_candidates": total,
            "note": "No matches can mean the ROM stores these graphics compressed or in a \
                     different bitplane/tile layout, not that the source is absent. \
                     Multiple matches can include coincidental hits — corroborate by \
                     reading more VRAM and re-searching a longer block.",
        })
    }

    /// `render_tiles`: decode a span of ROM/VRAM bytes AS tiles and return the
    /// result as a PNG IMAGE so Claude can SEE it and visually compare a candidate
    /// ROM region to what's on screen. This is the image-recognition evidence
    /// stream that complements `vram_to_rom` (raw byte-content match): it survives
    /// compressed / re-bitplaned graphics because it judges PIXELS, not bytes.
    ///
    /// `source` resolves to a byte span via the SAFE region path: a region NAME
    /// (exact), or the conveniences "rom"/"vram"/"memory" (first ROM/VRAM/any
    /// backed region). Bytes are cloned under a brief lock (via
    /// [`clone_region_bytes`], which goes through `safe_host_ptr` — NEVER a blind
    /// `from_raw_parts`), then decoded UNLOCKED. `len` is capped at
    /// [`MAX_RENDER_TILES_LEN`]. READ-ONLY: no write gate needed.
    ///
    /// Returns an image `Content` (base64 PNG, mime `image/png` — the same
    /// mechanism the `app://screen` resource uses to hand a viewable image to the
    /// MCP client) plus a small text `Content` describing dimensions/tile count.
    fn render_tiles(
        &self,
        source: &str,
        offset: usize,
        len: usize,
        format: TileFormat,
        tiles_per_row: usize,
    ) -> Result<CallToolResult, ErrorData> {
        let len = len.min(MAX_RENDER_TILES_LEN);
        let tiles_per_row = tiles_per_row.clamp(1, MAX_TILES_PER_ROW);

        // Resolve `source` to bytes — a live memory region OR the on-disk rom_file
        // (the cart bytes the core may not expose, e.g. NES CHR-ROM). Clones the
        // bytes out under a brief lock, then decodes unlocked.
        let (region_name, start, kind, all_bytes) = self
            .resolve_source_bytes(source)
            .map_err(|e| ErrorData::invalid_params(e, None))?;

        if offset >= all_bytes.len() {
            return Err(ErrorData::invalid_params(
                format!(
                    "offset 0x{offset:X} is beyond source '{region_name}' (len 0x{:X})",
                    all_bytes.len()
                ),
                None,
            ));
        }
        let end = (offset + len).min(all_bytes.len());
        let span = &all_bytes[offset..end];

        let img = decode_tiles_to_rgba(span, format, tiles_per_row).ok_or_else(|| {
            ErrorData::invalid_params(
                format!(
                    "not enough bytes at offset 0x{offset:X} of '{region_name}' to decode even one \
                     {format:?} tile"
                ),
                None,
            )
        })?;

        let png = rgba_to_png(&img.rgba, img.width, img.height)
            .ok_or_else(|| ErrorData::internal_error("tile PNG encoding failed", None))?;
        let b64 = base64_encode(&png);

        // Text part: orient the agent (what it's looking at).
        let info = json!({
            "source": source,
            "region": region_name,
            "kind": kind,
            "format": format!("{format:?}"),
            "region_addr_start": format!("0x{start:X}"),
            "byte_offset": format!("0x{offset:X}"),
            "abs_addr": format!("0x{:X}", start + offset),
            "bytes_decoded": span.len(),
            "tile_count": img.tile_count,
            "tiles_per_row": tiles_per_row,
            "image_px": format!("{}x{}", img.width, img.height),
            "palette": "grayscale ramp (real palette unknown; structure-only)",
            "note": "Visual evidence stream: compare this rendering to app://screen. \
                     Complements vram_to_rom (byte-content match) — use both for \
                     convergent evidence.",
        });
        let info_text = serde_json::to_string_pretty(&info)
            .map_err(|e| ErrorData::internal_error(format!("serialize error: {e}"), None))?;

        Ok(CallToolResult::success(vec![
            Content::text(info_text),
            Content::image(b64, "image/png"),
        ]))
    }

    /// Resolve a `render_tiles` `source` token to a concrete region name. Accepts
    /// an exact region name, or the conveniences "rom"/"vram"/"memory" (first
    /// ROM/VRAM/any backed region, respectively). Returns an error string listing
    /// the available regions when nothing matches.
    fn resolve_render_source(&self, source: &str) -> Result<String, String> {
        let ds = self
            .debug
            .lock()
            .map_err(|_| "debug state lock poisoned".to_string())?;
        // Exact name match first (case-insensitive).
        if let Some(r) = ds
            .memory_regions
            .iter()
            .find(|r| r.name.eq_ignore_ascii_case(source))
        {
            return Ok(r.name.clone());
        }
        // Convenience aliases.
        let want_kind = match source.trim().to_ascii_lowercase().as_str() {
            "rom" => Some("ROM"),
            "vram" => Some("VRAM"),
            "memory" | "" => None, // first backed region of any kind
            _ => {
                let names: Vec<String> =
                    ds.memory_regions.iter().map(|r| r.name.clone()).collect();
                return Err(format!(
                    "no region matches source '{source}'. Use a region name, or rom/vram/memory. \
                     Available: {}",
                    names.join(", ")
                ));
            }
        };
        let pick = ds.memory_regions.iter().find(|r| {
            let backed = read_region_bytes(r, 0, 1).is_some();
            backed && want_kind.map(|k| r.region_type() == k).unwrap_or(true)
        });
        match pick {
            Some(r) => Ok(r.name.clone()),
            None => {
                let names: Vec<String> =
                    ds.memory_regions.iter().map(|r| r.name.clone()).collect();
                Err(format!(
                    "no readable {} region found for source '{source}'. Available: {}",
                    want_kind.unwrap_or("backed"),
                    names.join(", ")
                ))
            }
        }
    }

    /// If `source` selects the on-disk ROM FILE, return its optional iNES
    /// sub-part: `Some(None)` = the whole cart, `Some(Some("chr"))` = a named span
    /// (`header`/`prg`/`chr`), `None` = not a rom_file source (a live region). The
    /// ROM file lets tools decode content the running core does NOT expose in
    /// memory — e.g. NES CHR-ROM graphics. Accepts a few base spellings.
    fn rom_file_part(source: &str) -> Option<Option<String>> {
        let s = source.trim().to_ascii_lowercase();
        let (base, part) = match s.split_once(':') {
            Some((b, p)) => (b.trim().to_string(), Some(p.trim().to_string())),
            None => (s, None),
        };
        match base.as_str() {
            "rom_file" | "romfile" | "rom-file" | "file" => Some(part),
            _ => None,
        }
    }


    /// Whether `source` selects the on-disk ROM FILE (whole cart or a named span).
    #[cfg_attr(not(test), allow(dead_code))] // exercised by the test suite
    fn is_rom_file_source(source: &str) -> bool {
        Self::rom_file_part(source).is_some()
    }

    /// Slice the ROM file to an iNES sub-part (`header`/`prg`/`chr`) or the whole
    /// cart. Returns `(display_name, file_offset, kind, bytes)`. Errors honestly
    /// for a CHR-RAM cart's `:chr` (no CHR-ROM in the file) or a non-iNES file.
    fn slice_rom_file(
        &self,
        name: String,
        bytes: Vec<u8>,
        part: Option<&str>,
    ) -> Result<(String, usize, String, Vec<u8>), String> {
        let part = match part {
            None | Some("") => {
                return Ok((format!("ROM file ({name})"), 0, "ROMFILE".to_string(), bytes))
            }
            Some(p) => p,
        };
        let info = parse_ines(&bytes).ok_or_else(|| {
            format!("rom_file:{part} needs an iNES (.nes) file, but this ROM has no iNES header")
        })?;
        let (start, end, kind) = match part {
            "header" => (0usize, 16usize.min(bytes.len()), "ROMFILE:HEADER"),
            "prg" => (
                info.prg_offset,
                info.prg_offset + info.prg_rom_size,
                "ROMFILE:PRG",
            ),
            "chr" => {
                if info.chr_is_ram {
                    return Err("this cart uses CHR-RAM: there is no CHR-ROM in the file to \
                                decode (the graphics may live compressed in PRG-ROM, or only \
                                appear in live CHR-RAM via the core)"
                        .to_string());
                }
                (
                    info.chr_offset,
                    info.chr_offset + info.chr_rom_size,
                    "ROMFILE:CHR",
                )
            }
            other => {
                return Err(format!(
                    "unknown rom_file part '{other}'; use header | prg | chr (or plain \
                     rom_file for the whole cart)"
                ))
            }
        };
        let end = end.min(bytes.len());
        let start = start.min(end);
        Ok((
            format!("ROM file ({name}):{part}"),
            start,
            kind.to_string(),
            bytes[start..end].to_vec(),
        ))
    }

    /// Fetch the loaded ROM file's bytes for the `rom_file` source: prefer the
    /// bytes retained at load, else re-read the retained path (need_fullpath cores
    /// never read the bytes into memory). Returns (display_name, bytes). The lock
    /// is dropped before any disk read.
    fn rom_file_bytes(&self) -> Result<(String, Vec<u8>), String> {
        let (name, bytes, path) = {
            let ds = self
                .debug
                .lock()
                .map_err(|_| "debug state lock poisoned".to_string())?;
            (ds.rom_name.clone(), ds.rom_bytes.clone(), ds.rom_path.clone())
        };
        let label = name.unwrap_or_else(|| "rom".to_string());
        if let Some(b) = bytes {
            if !b.is_empty() {
                return Ok((label, b));
            }
        }
        if let Some(p) = path {
            return std::fs::read(&p)
                .map(|b| (label, b))
                .map_err(|e| format!("failed to read ROM file {}: {e}", p.display()));
        }
        Err("no ROM file available (ROM not loaded, or a need_fullpath core whose \
             path wasn't retained)"
            .to_string())
    }

    /// Resolve a tool `source` token to its bytes — handling BOTH live memory
    /// regions (exact NAME, or rom/vram/memory) AND the on-disk `rom_file`
    /// pseudo-source. Returns `(display_name, region_addr_start, kind, bytes)`;
    /// `start`/`kind` are `0`/`"ROMFILE"` for the file source. The error string is
    /// ready to surface to the client (the `REGION_UNBACKED` sentinel is mapped to
    /// a friendly message here so both render_tiles and scan_regions share it).
    fn resolve_source_bytes(
        &self,
        source: &str,
    ) -> Result<(String, usize, String, Vec<u8>), String> {
        if let Some(part) = Self::rom_file_part(source) {
            let (name, bytes) = self.rom_file_bytes()?;
            return self.slice_rom_file(name, bytes, part.as_deref());
        }
        let region_name = self.resolve_render_source(source)?;
        match self.clone_region_bytes(&region_name) {
            Ok((start, kind, bytes)) => Ok((region_name, start, kind, bytes)),
            Err(e) if e.starts_with(REGION_UNBACKED) => Err(format!(
                "region '{region_name}' is declared but not backed by readable memory \
                 (virtual descriptor)"
            )),
            Err(e) => Err(e),
        }
    }

    /// `rom_info`: parse the loaded ROM file's iNES / NES 2.0 header and report
    /// the layout — mapper, PRG/CHR sizes, and the exact FILE OFFSETS of PRG-ROM
    /// and CHR-ROM — so a caller can point `render_tiles`/`scan_regions` at
    /// `rom_file:chr` (or a raw offset) without hand-computing it. READ-ONLY.
    /// Non-iNES ROMs (other systems) return a clear note rather than an error, so
    /// the raw `rom_file` byte source still works.
    fn rom_info(&self) -> Result<CallToolResult, ErrorData> {
        let (name, bytes) = self
            .rom_file_bytes()
            .map_err(|e| ErrorData::invalid_params(e, None))?;

        let v = match parse_ines(&bytes) {
            Some(i) => json!({
                "name": name,
                "format": if i.is_nes2 { "NES 2.0" } else { "iNES" },
                "system": "nes",
                "file_len": i.file_len,
                "mapper": i.mapper,
                "submapper": i.submapper,
                "mirroring": i.mirroring,
                "battery": i.battery,
                "has_trainer": i.has_trainer,
                "prg_rom_size": i.prg_rom_size,
                "chr_rom_size": i.chr_rom_size,
                "chr_is_ram": i.chr_is_ram,
                "chr_ram_size": i.chr_ram_size,
                "prg_offset": format!("0x{:X}", i.prg_offset),
                "chr_offset": if i.chr_is_ram { "n/a (CHR-RAM)".to_string() } else { format!("0x{:X}", i.chr_offset) },
                "hint": if i.chr_is_ram {
                    "CHR-RAM cart: no CHR-ROM in the file. Tiles may live (compressed) in PRG-ROM, \
                     or only in live CHR-RAM via the core.".to_string()
                } else {
                    format!(
                        "Decode the graphics with: render_tiles source=rom_file:chr format=nes_chr \
                         (CHR-ROM is {} tiles at file 0x{:X}). scan_regions source=rom_file shows \
                         the PRG/CHR/data split.",
                        i.chr_rom_size / 16,
                        i.chr_offset
                    )
                },
            }),
            None => json!({
                "name": name,
                "format": "non-iNES",
                "file_len": bytes.len(),
                "note": "No iNES (.nes) header. The raw `rom_file` source still works for \
                         render_tiles/scan_regions, but header/prg/chr spans and NES layout \
                         fields are unavailable for this system.",
            }),
        };
        Ok(CallToolResult::success(vec![Self::json_content(&v)?]))
    }

    /// `scan_regions`: the STRUCTURE / statistical-signature evidence stream.
    /// Window a region's bytes and propose what KIND each span looks like
    /// (padding / text / packed / table / graphics / code) from cheap signatures
    /// (Shannon entropy, byte-histogram, printable/fill fractions), so the agent
    /// can ORIENT inside an unknown ROM before zeroing in with the precise
    /// streams (`render_tiles`, `vram_to_rom`, the PC heatmap).
    ///
    /// `source` resolves via the same SAFE region path as `render_tiles` (exact
    /// region name, or "rom"/"vram"/"memory"). Bytes are cloned under a brief lock
    /// (via [`clone_region_bytes`] → `safe_host_ptr`), capped at [`MAX_SCAN_LEN`],
    /// then analysed UNLOCKED. READ-ONLY: no write gate. The proposals are
    /// HEURISTICS (`confidence` = `guess`/`likely`) — convergent evidence, not a
    /// verdict.
    fn scan_regions(&self, source: &str, window: usize) -> Result<CallToolResult, ErrorData> {
        let window = window.clamp(MIN_SCAN_WINDOW, MAX_SCAN_WINDOW);

        // Resolve `source` to bytes — a live memory region OR the on-disk rom_file.
        let (region_name, start, kind, all_bytes) = self
            .resolve_source_bytes(source)
            .map_err(|e| ErrorData::invalid_params(e, None))?;

        // Bound the work: scan at most MAX_SCAN_LEN bytes (the prefix), flag if
        // the region is larger so the agent knows the tail wasn't analysed.
        let truncated = all_bytes.len() > MAX_SCAN_LEN;
        let span = &all_bytes[..all_bytes.len().min(MAX_SCAN_LEN)];

        let candidates = scan_buffer(span, window);

        // Project each candidate to absolute guest addresses and a compact shape.
        let regions: Vec<Value> = candidates
            .iter()
            .map(|c| {
                json!({
                    "kind": c.kind,
                    "confidence": c.confidence,
                    "addr_start": format!("0x{:X}", start + c.start),
                    "addr_end": format!("0x{:X}", start + c.end),
                    "offset": format!("0x{:X}", c.start),
                    "len": c.len,
                    "windows": c.windows,
                    "mean_entropy": (c.mean_entropy * 100.0).round() / 100.0,
                    "reason": c.reason,
                })
            })
            .collect();

        // Per-kind byte tally so the agent gets a one-glance composition.
        let mut tally: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
        for c in &candidates {
            *tally.entry(c.kind).or_insert(0) += c.len;
        }
        let composition: Map<String, Value> = tally
            .into_iter()
            .map(|(k, bytes)| (k.to_string(), json!(bytes)))
            .collect();

        let out = json!({
            "source": source,
            "region": region_name,
            "region_kind": kind,
            "region_addr_start": format!("0x{:X}", start),
            "bytes_scanned": span.len(),
            "region_size": all_bytes.len(),
            "truncated": truncated,
            "window": window,
            "candidate_count": candidates.len(),
            "composition_bytes": composition,
            "candidates": regions,
            "note": "Heuristic STRUCTURE stream: entropy/histogram signatures propose a KIND \
                     per span. Corroborate before trusting — render_tiles to eyeball 'graphics', \
                     the PC heatmap to confirm 'code', vram_to_rom for content match. Convergence \
                     promotes a finding to confirmed.",
        });

        Ok(CallToolResult::success(vec![Self::json_content(&out)?]))
    }

    /// Set the `paused` control flag. Safe — cannot corrupt memory.
    fn set_paused(&self, paused: bool) -> Value {
        if let Ok(mut ds) = self.debug.lock() {
            ds.paused = paused;
            json!({ "ok": true, "paused": paused })
        } else {
            json!({ "ok": false, "error": "lock poisoned" })
        }
    }

    /// `press_buttons`: hold one or more controller buttons (port 0) for `frames`
    /// emulated frames, so an agent can drive the game — advance menus, start a
    /// match, perform moves — in headless mode where there's no keyboard. Safe: it
    /// only feeds the controller, it cannot corrupt memory, so it is NOT behind the
    /// write gate. Buttons are held simultaneously (e.g. ["down","b"]); call again
    /// to chain inputs. Resume/run must be active for frames to advance.
    fn press_buttons(&self, names: &[&str], frames: u32, port: usize) -> Value {
        if port > 1 {
            return json!({ "ok": false, "error": "`port` must be 0 (P1) or 1 (P2)" });
        }
        let frames = frames.clamp(1, MAX_INPUT_HOLD_FRAMES) as u16;
        let mut set = Vec::new();
        let mut unknown = Vec::new();
        // Resolve names first so we can report errors without holding the lock.
        let mut indices = Vec::new();
        for n in names {
            match joypad_button_index(n) {
                Some(i) => {
                    indices.push(i);
                    set.push(n.to_ascii_lowercase());
                }
                None => unknown.push(n.to_string()),
            }
        }
        if !unknown.is_empty() {
            return json!({
                "ok": false,
                "error": format!("unknown button(s): {}. Valid: {}", unknown.join(", "), JOYPAD_BUTTON_LIST),
            });
        }
        let paused = match self.debug.lock() {
            Ok(mut ds) => {
                let arr = if port == 1 { &mut ds.injected_input2 } else { &mut ds.injected_input };
                for i in indices {
                    arr[i] = frames;
                }
                ds.paused
            }
            Err(_) => return json!({ "ok": false, "error": "lock poisoned" }),
        };
        json!({
            "ok": true,
            "pressed": set,
            "frames": frames,
            "port": port,
            "note": if paused {
                "buttons queued, but emulation is PAUSED — call resume so frames advance"
            } else {
                "holding for the requested frames, then auto-released"
            },
        })
    }

    /// `hold_buttons`: assert one or more controller buttons on `port` on EVERY
    /// fold until [`release_buttons`](Self::release_buttons) clears them —
    /// unlike `press_buttons`'s frame-countdown, a held button does not expire
    /// and does not drain while the emulation is paused (see
    /// `DebugState::held_input`). Idempotent: calling this again REPLACES the
    /// port's held set rather than adding to it, so `hold_buttons(0, [])`
    /// releases everything on that port. Safe — input-only, no memory writes —
    /// so like `press_buttons` it is NOT behind the write gate.
    fn hold_buttons(&self, names: &[&str], port: usize) -> Value {
        if port > 1 {
            return json!({ "ok": false, "error": "`port` must be 0 (P1) or 1 (P2)" });
        }
        let mut bits = [false; 12];
        let mut set = Vec::new();
        let mut unknown = Vec::new();
        for n in names {
            match joypad_button_index(n) {
                Some(i) => {
                    bits[i] = true;
                    set.push(n.to_ascii_lowercase());
                }
                None => unknown.push(n.to_string()),
            }
        }
        if !unknown.is_empty() {
            return json!({
                "ok": false,
                "error": format!("unknown button(s): {}. Valid: {}", unknown.join(", "), JOYPAD_BUTTON_LIST),
            });
        }
        match self.debug.lock() {
            Ok(mut ds) => ds.set_held_input(port, bits),
            Err(_) => return json!({ "ok": false, "error": "lock poisoned" }),
        }
        json!({
            "ok": true,
            "held": set,
            "port": port,
            "note": "asserted on every fold until release_buttons clears it",
        })
    }

    /// `release_buttons`: clear the named buttons (or, when `names` is empty,
    /// the ENTIRE held set) from `port`'s held mask. Does not touch any
    /// in-flight `press_buttons` countdown.
    fn release_buttons(&self, names: &[&str], port: usize) -> Value {
        if port > 1 {
            return json!({ "ok": false, "error": "`port` must be 0 (P1) or 1 (P2)" });
        }
        if names.is_empty() {
            match self.debug.lock() {
                Ok(mut ds) => ds.clear_held_input(port, None),
                Err(_) => return json!({ "ok": false, "error": "lock poisoned" }),
            }
            return json!({ "ok": true, "released": "all", "port": port });
        }
        let mut idxs = Vec::new();
        let mut set = Vec::new();
        let mut unknown = Vec::new();
        for n in names {
            match joypad_button_index(n) {
                Some(i) => {
                    idxs.push(i);
                    set.push(n.to_ascii_lowercase());
                }
                None => unknown.push(n.to_string()),
            }
        }
        if !unknown.is_empty() {
            return json!({
                "ok": false,
                "error": format!("unknown button(s): {}. Valid: {}", unknown.join(", "), JOYPAD_BUTTON_LIST),
            });
        }
        match self.debug.lock() {
            Ok(mut ds) => ds.clear_held_input(port, Some(&idxs)),
            Err(_) => return json!({ "ok": false, "error": "lock poisoned" }),
        }
        json!({ "ok": true, "released": set, "port": port })
    }

    /// `get_input`: report `port`'s input state three ways — `asserted` is
    /// what the NEXT fold will feed the core (held ORed with any live
    /// `press_buttons` countdown, via `DebugState::peek_injected_input`,
    /// non-consuming); `folded` is the LAST TICK's fold
    /// (`input_state`/`input_state2`) — this is refreshed every host-loop
    /// tick regardless of whether that tick's frame actually ran, so while
    /// paused it can race back to matching the held set even when the last
    /// EXECUTED frame briefly saw something else; `executed` is the fix for
    /// that — `DebugState::last_executed_input(2)`, updated ONLY on frames
    /// that actually ran `core.run()`, atomically with the decision to run,
    /// so it can't be overwritten by a later non-executing tick's re-fold.
    /// Use `executed` when you need to know what a SPECIFIC landed frame
    /// (e.g. from `run_frames`) actually saw.
    fn get_input(&self, port: usize) -> Value {
        if port > 1 {
            return json!({ "ok": false, "error": "`port` must be 0 (P1) or 1 (P2)" });
        }
        let ds = match self.debug.lock() {
            Ok(g) => g,
            Err(_) => return json!({ "ok": false, "error": "lock poisoned" }),
        };
        let asserted = ds.peek_injected_input(port);
        let folded = if port == 1 { ds.input_state2 } else { ds.input_state };
        let held = if port == 1 { ds.held_input2 } else { ds.held_input };
        let executed = if port == 1 { ds.last_executed_input2 } else { ds.last_executed_input };
        drop(ds);
        json!({
            "ok": true,
            "port": port,
            "asserted_mask": format!("0x{:03X}", mask_from_bits(&asserted)),
            "asserted_buttons": button_names(&asserted),
            "held_buttons": button_names(&held),
            "folded_mask": format!("0x{:03X}", mask_from_bits(&folded)),
            "folded_buttons": button_names(&folded),
            "executed_mask": format!("0x{:03X}", mask_from_bits(&executed)),
            "executed_buttons": button_names(&executed),
            "note": "asserted = what the NEXT fold will feed the core (held + any press_buttons \
                     countdown remaining); folded = the LAST TICK's fold (input_state/input_state2 \
                     — refreshed every host-loop tick whether or not that tick's frame ran, also \
                     includes keyboard/pad in windowed mode); executed = what the LAST FRAME THAT \
                     ACTUALLY RAN core.run() saw (sticky — only changes on a real frame, so it is \
                     safe to read after a `step`/`run_frames` call without racing a later tick)",
        })
    }

    // ── input-slot record/playback (task A2) ────────────────────────────────
    //
    // Named slots capturing both ports' folded per-frame input, replayable
    // deterministically — the frame lab's determinism instrument AND a
    // reproducible bug-repro format. See `playback.rs`'s module doc for the
    // precedence rule against the training dummy and exactly what is/isn't
    // guaranteed deterministic. GATED behind enable_writes per task A2's
    // explicit requirement — unlike hold_buttons/press_buttons (which predate
    // this feature and stay ungated), these are asked-for as gated tools.

    /// `record_inputs`: start or stop capturing BOTH ports into a named slot
    /// (`shadow/inputs/<family>/<name>.slot.json`).
    fn record_inputs(&self, action: &str, name: Option<&str>) -> Value {
        if let Err(e) = self.check_writes_armed() {
            return json!({ "ok": false, "error": e });
        }
        let mut ds = match self.debug.lock() {
            Ok(g) => g,
            Err(_) => return json!({ "ok": false, "error": "lock poisoned" }),
        };
        match action {
            "start" => {
                let Some(name) = name else {
                    return json!({ "ok": false, "error": "`name` is required for action=\"start\"" });
                };
                match crate::playback::start_recording(&mut ds, name, crate::profile::current()) {
                    Ok(()) => {
                        ds.recording_note = Some(format!("recording '{name}'"));
                        json!({ "ok": true, "recording": name })
                    }
                    Err(e) => json!({ "ok": false, "error": e }),
                }
            }
            "stop" => match crate::playback::stop_recording(&mut ds) {
                Ok((path, frames)) => {
                    let path_s = path.display().to_string();
                    ds.recording_note = Some(format!("stopped — {frames} frames → {path_s}"));
                    json!({ "ok": true, "path": path_s, "frames": frames })
                }
                Err(e) => json!({ "ok": false, "error": e }),
            },
            other => json!({
                "ok": false,
                "error": format!("`action` must be \"start\" or \"stop\", got '{other}'")
            }),
        }
    }

    /// `play_inputs`: start or stop replaying a named slot onto one or both
    /// ports. `trigger` is `"manual"` (begins on the next real frame — frame-
    /// exact only when paired with pause→step, see `playback.rs`'s module
    /// doc) or `"round_start"` (begins on the fight gate's next closed→open
    /// transition — deterministic from a pre-round save state).
    fn play_inputs(&self, action: &str, name: Option<&str>, port: &str, trigger: &str) -> Value {
        if let Err(e) = self.check_writes_armed() {
            return json!({ "ok": false, "error": e });
        }
        let mut ds = match self.debug.lock() {
            Ok(g) => g,
            Err(_) => return json!({ "ok": false, "error": "lock poisoned" }),
        };
        match action {
            "start" => {
                let Some(name) = name else {
                    return json!({ "ok": false, "error": "`name` is required for action=\"start\"" });
                };
                let port = match port {
                    "p1" => crate::debug::PlaybackPort::P1,
                    "p2" => crate::debug::PlaybackPort::P2,
                    "both" | "" => crate::debug::PlaybackPort::Both,
                    other => {
                        return json!({
                            "ok": false,
                            "error": format!("`port` must be \"p1\", \"p2\", or \"both\", got '{other}'")
                        })
                    }
                };
                let trigger = match trigger {
                    "manual" | "" => crate::debug::PlaybackTrigger::Manual,
                    "round_start" => crate::debug::PlaybackTrigger::RoundStart,
                    other => {
                        return json!({
                            "ok": false,
                            "error": format!("`trigger` must be \"manual\" or \"round_start\", got '{other}'")
                        })
                    }
                };
                match crate::playback::start_playback(&mut ds, name, port, trigger, crate::profile::current()) {
                    Ok(frames) => {
                        ds.playback_note = Some(format!("armed '{name}' ({frames} frames)"));
                        json!({ "ok": true, "name": name, "frames": frames })
                    }
                    Err(e) => json!({ "ok": false, "error": e }),
                }
            }
            "stop" => match crate::playback::stop_playback(&mut ds) {
                Ok(()) => {
                    ds.playback_note = Some("stopped".to_string());
                    json!({ "ok": true, "stopped": true })
                }
                Err(e) => json!({ "ok": false, "error": e }),
            },
            other => json!({
                "ok": false,
                "error": format!("`action` must be \"start\" or \"stop\", got '{other}'")
            }),
        }
    }

    /// `list_input_slots`: list every slot under the loaded family's
    /// `shadow/inputs/<family>/` directory. Read-only, UNGATED.
    fn list_input_slots(&self) -> Value {
        let family = crate::profile::current().family.family.clone();
        let slots = crate::playback::list_slots(&family);
        json!({
            "ok": true,
            "family": family,
            "dir": crate::playback::slots_dir(&family).display().to_string(),
            "slots": serde_json::to_value(&slots).unwrap_or_else(|_| json!([])),
        })
    }

    // ── signal hunt (docs/signal-hunt.md — NORMATIVE) ──────────────────────
    //
    // The four tools below automate the event-marked differential RAM protocol
    // that had been hand-scripted once per hunt. None of them writes anything
    // to the game or to a profile, so none is behind the write gate: marking
    // reads the frame counter and the gate, analysis is pure arithmetic over
    // snapshots the sampler already took.

    /// `hunt_mark`: record a labeled moment (§2). Pins the `mark-PRE` snapshot
    /// out of the ring immediately and schedules the `mark+POST` capture.
    fn hunt_mark(&self, label: &str) -> Value {
        match crate::hunt::mark(&self.debug, label) {
            Ok(msg) => json!({ "ok": true, "message": msg, "status": crate::hunt::status() }),
            Err(e) => json!({ "ok": false, "error": e }),
        }
    }

    /// `hunt_configure`: scope the hunt region and set the window knobs (§3).
    #[allow(clippy::too_many_arguments)]
    fn hunt_configure(
        &self,
        blocks: bool,
        extra: Option<(u32, u32)>,
        ring_frames: Option<usize>,
        pre: Option<u64>,
        post: Option<u64>,
        include_idle: Option<bool>,
        enabled: Option<bool>,
    ) -> Value {
        match crate::hunt::configure(blocks, extra, ring_frames, pre, post, include_idle, enabled) {
            Ok(msg) => json!({ "ok": true, "message": msg, "status": crate::hunt::status() }),
            // §3: a refusal is a first-class answer, not a fallback to a
            // truncated region.
            Err(e) => json!({ "ok": false, "error": e }),
        }
    }

    /// `hunt_analyze`: the §4 kernel plus the §5 evidence-doc export and the
    /// §6 honesty fields, all in one payload.
    fn hunt_analyze(&self, event_label: &str, control_label: Option<&str>) -> Value {
        match crate::hunt::run_analysis(event_label, control_label) {
            Ok(a) => {
                let md = crate::hunt::export_markdown(&a);
                let mut v = serde_json::to_value(&a).unwrap_or_else(|e| json!({ "error": e.to_string() }));
                if let Some(obj) = v.as_object_mut() {
                    obj.insert("ok".into(), json!(true));
                    obj.insert("evidence_markdown".into(), json!(md));
                    obj.insert(
                        "profile_note".into(),
                        json!("This tool NEVER writes a profile. Candidates are hypotheses; \
                               promotion requires a write-test."),
                    );
                }
                v
            }
            Err(e) => json!({ "ok": false, "error": e }),
        }
    }

    /// `hunt_reset`: drop every mark and the ring; keep the configuration.
    fn hunt_reset(&self) -> Value {
        json!({ "ok": true, "message": crate::hunt::reset(), "status": crate::hunt::status() })
    }

    /// Block until `DebugState::step_generation` advances past `start_gen`,
    /// waking on the SAME `notify_all` `Frontend::run_frame` fires once a
    /// frame's post-processing is entirely done — not a moved `frame_count`
    /// observed via polling, which is what made the old fire-and-forget
    /// `step` need an 8ms settle (docs/frames.md §3 precondition 6 ("let the frame finish")). Bounded by `timeout`;
    /// returns `Ok(None)` on timeout instead of blocking forever, so a
    /// wedged emulation thread surfaces as a clear error, not a hang.
    fn wait_for_next_frame(&self, start_gen: u64, timeout: Duration) -> Result<Option<u64>, &'static str> {
        let mut ds = self.debug.lock().map_err(|_| "lock poisoned")?;
        if ds.step_generation != start_gen {
            return Ok(Some(ds.frame_count));
        }
        let cv = ds.frame_cv.clone(); // clone out of the guard before moving it into wait_timeout
        let deadline = Instant::now() + timeout;
        loop {
            let now = Instant::now();
            if now >= deadline {
                return Ok(None);
            }
            let (guard, _) = cv.wait_timeout(ds, deadline - now).map_err(|_| "lock poisoned")?;
            ds = guard;
            if ds.step_generation != start_gen {
                return Ok(Some(ds.frame_count));
            }
        }
    }

    /// Block until `DebugState::fold_generation` advances past `start_gen`, or
    /// `timeout` expires. This is `run_frames`' guarantee that the port masks
    /// it just set have been folded into `Frontend::callback_context.input_state`
    /// (`main.rs`'s headless step (a0) / windowed `read_input`) at least once
    /// BEFORE it arms `step_batch_remaining` — closing the race documented on
    /// `run_frames` itself: setting held masks and arming the batch used to
    /// happen in a single lock acquisition, with no guarantee the host loop's
    /// separate fold-then-gate-check pair had picked up the new masks before
    /// the gate opened. Since the fold runs unconditionally every host-loop
    /// tick (not gated on `paused`), the FIRST fold observed after this call's
    /// caller mutates `held_input`/`held_input2` is guaranteed — by mutex
    /// total order alone, no timing assumption — to have read the new values:
    /// the mutation happens-before this function's lock is released, and any
    /// fold's lock acquisition that observes a bumped generation happened
    /// strictly after that release.
    fn wait_for_fold(&self, start_gen: u64, timeout: Duration) -> Result<Option<u64>, &'static str> {
        let mut ds = self.debug.lock().map_err(|_| "lock poisoned")?;
        if ds.fold_generation != start_gen {
            return Ok(Some(ds.fold_generation));
        }
        let cv = ds.frame_cv.clone();
        let deadline = Instant::now() + timeout;
        loop {
            let now = Instant::now();
            if now >= deadline {
                return Ok(None);
            }
            let (guard, _) = cv.wait_timeout(ds, deadline - now).map_err(|_| "lock poisoned")?;
            ds = guard;
            if ds.fold_generation != start_gen {
                return Ok(Some(ds.fold_generation));
            }
        }
    }

    /// `run_frames`' counterpart to [`wait_for_next_frame`](Self::wait_for_next_frame):
    /// blocks until `count` frames' worth of `step_generation` have landed, or
    /// `timeout` expires. Returns `(frames_landed, end_frame, timed_out)`; on
    /// timeout it also clears `step_batch_remaining` so a wedged wait doesn't
    /// leave a dangling auto-step budget for later frames to silently burn
    /// through unsupervised.
    ///
    /// Deliberately does NOT also exit early on `step_batch_remaining == 0` —
    /// that field is decremented at the START of `Frontend::run_frame` (before
    /// `core.run()`), while `step_generation` is only bumped at the very END
    /// (after all post-processing). A waiter that also treated
    /// `step_batch_remaining == 0` as "done" could observe the LAST frame's
    /// decrement (start of its `run_frame` call) before that same frame's
    /// generation bump (end of the same call) and return one frame short —
    /// measured live as `run_frames(60)` reporting `landed: 59`. Generation is
    /// the only completion signal; the two counters' independent lock windows
    /// must not be cross-checked against each other.
    fn wait_for_batch(&self, start_gen: u64, count: u32, timeout: Duration) -> Result<(u64, u64, bool), &'static str> {
        let mut ds = self.debug.lock().map_err(|_| "lock poisoned")?;
        let cv = ds.frame_cv.clone();
        let deadline = Instant::now() + timeout;
        loop {
            let landed = ds.step_generation.wrapping_sub(start_gen).min(count as u64);
            if landed >= count as u64 {
                return Ok((landed, ds.frame_count, false));
            }
            let now = Instant::now();
            if now >= deadline {
                ds.step_batch_remaining = 0;
                let landed = ds.step_generation.wrapping_sub(start_gen).min(count as u64);
                return Ok((landed, ds.frame_count, true));
            }
            let (guard, _) = cv.wait_timeout(ds, deadline - now).map_err(|_| "lock poisoned")?;
            ds = guard;
        }
    }

    /// Advance emulation by exactly one frame, SYNCHRONOUSLY: unlike the old
    /// fire-and-forget `step_one` flag-set, this only returns once that frame
    /// has completely finished (`Frontend::run_frame`'s full post-processing
    /// done, per docs/frames.md §3 precondition 6 ("let the frame finish")) — not when a counter merely moved. An
    /// immediately-following `hold_buttons`/read is safe once this returns.
    /// Backward compatible: existing callers that poll `get_state` afterwards
    /// still work, they just observe it already landed.
    fn step(&self) -> Value {
        let start_gen = match self.debug.lock() {
            Ok(mut ds) => {
                let g = ds.step_generation;
                ds.step_one = true;
                g
            }
            Err(_) => return json!({ "ok": false, "error": "lock poisoned" }),
        };
        match self.wait_for_next_frame(start_gen, STEP_WAIT_TIMEOUT) {
            Ok(Some(frame_count)) => json!({
                "ok": true,
                "stepped": true,
                "landed": true,
                "frame_count": frame_count,
            }),
            Ok(None) => json!({
                "ok": false,
                "stepped": true,
                "landed": false,
                "error": format!(
                    "timed out after {:?} waiting for the emulation thread to finish the frame \
                     (it may be wedged) — step_one is still armed and will consume the next \
                     frame that does run",
                    STEP_WAIT_TIMEOUT,
                ),
            }),
            Err(e) => json!({ "ok": false, "error": e }),
        }
    }

    /// `run_frames`: advance `count` frames SYNCHRONOUSLY in ONE call — the
    /// batch counterpart to `step`, collapsing a replay segment from N round
    /// trips into one (the frame lab measured a confirmed, settled `step` at
    /// ~41ms; this exists so a whole segment doesn't pay that per frame).
    ///
    /// Requires the emulator to already be PAUSED — same precondition as the
    /// documented pause→step frame-exact workflow (CLAUDE.md's MCP/agent-
    /// workflow section). Refuses otherwise rather than silently pausing/
    /// resuming around the caller, which would be a surprising side effect
    /// for a "just advance frames" tool.
    ///
    /// `port0`/`port1`, when given, REPLACE that port's held set for the
    /// whole run — identical semantics to `hold_buttons` (not additive), and
    /// they stay held after the call returns (`release_buttons` to clear).
    /// Ports not mentioned keep whatever was already held.
    ///
    /// ORDERING GUARANTEE (closes a measured race — 13/200 spurious results on
    /// a live rig before this fix): setting the masks and arming
    /// `step_batch_remaining` are DELIBERATELY two separate lock acquisitions
    /// with a [`wait_for_fold`](Self::wait_for_fold) in between, not one. The
    /// host loop folds input (`main.rs`'s headless step (a0) / windowed
    /// `read_input`) and checks the pause/batch gate (`Frontend::run_frame`)
    /// in two SEPARATE lock acquisitions of its own, once per tick. Setting
    /// the masks and arming the batch together in a single critical section
    /// left a window: the host loop could fold the OLD masks and then, in the
    /// very same tick, see the batch just got armed and run frame 1 on that
    /// stale fold — the attacker's move landing one frame late, silently.
    /// Waiting for a CONFIRMED fold (fold_generation advancing past a value
    /// snapshotted before the masks were mutated) before arming the batch
    /// guarantees the batch's first counted frame sees the new masks: the
    /// mutation happens-before the wait's lock release, so the first fold
    /// observed afterward is provably a fold of the new values, and every
    /// fold from then on reads the same (unchanged) held set. Only paid when
    /// `port0`/`port1` is actually given — the no-mask path (already measured
    /// at 0/200) is untouched. Residual hazard: this is still "single agent
    /// per MCP session" like the rest of this server — a concurrent
    /// `hold_buttons`/`run_frames` from another session between the fold
    /// confirmation and the batch arming could overwrite the just-confirmed
    /// masks; not defended against, same as the pre-existing
    /// `step_batch_remaining` sharing noted below. If the emulator is resumed
    /// or re-paused between the two lock acquisitions, the second one re-
    /// checks `paused` and refuses rather than silently arming a batch against
    /// a state that changed underneath it.
    ///
    /// A `load_state` landing mid-run is NOT special-cased: `Frontend::
    /// frame_count` is a host-side counter that is never part of the
    /// save-state blob (retro_(un)serialize never touches it), so it stays
    /// monotonic straight through a load — only the CONTENT of frames run
    /// after the load reflects the restored machine state. `end_frame` is
    /// therefore always `start_frame + landed`, even across a mid-batch load.
    ///
    /// Not mutually exclusive with a concurrent `step`/`run_frames` from
    /// another session: both share the same `step_batch_remaining` counter,
    /// same as `hold_buttons`/`press_buttons` already race on shared input
    /// state today. The frame lab's protocol is single-agent-per-session by
    /// convention, same as the rest of this server.
    fn run_frames(&self, count: u32, port0: Option<&[&str]>, port1: Option<&[&str]>) -> Value {
        if count == 0 {
            return json!({ "ok": false, "error": "`count` must be >= 1" });
        }
        if count > MAX_RUN_FRAMES {
            return json!({
                "ok": false,
                "error": format!(
                    "`count` must be <= {MAX_RUN_FRAMES} (~10s at 60fps, keeps a single call \
                     from making the server unresponsive) — issue multiple calls for a longer \
                     segment",
                ),
            });
        }

        let mut unknown: Vec<String> = Vec::new();
        let resolve = |names: &[&str], unknown: &mut Vec<String>| -> [bool; 12] {
            let mut bits = [false; 12];
            for n in names {
                match joypad_button_index(n) {
                    Some(i) => bits[i] = true,
                    None => unknown.push(n.to_string()),
                }
            }
            bits
        };
        let bits0 = port0.map(|n| resolve(n, &mut unknown));
        let bits1 = port1.map(|n| resolve(n, &mut unknown));
        if !unknown.is_empty() {
            return json!({
                "ok": false,
                "error": format!("unknown button(s): {}. Valid: {}", unknown.join(", "), JOYPAD_BUTTON_LIST),
            });
        }

        // Phase 1: validate paused, then (if masks were given) mutate held
        // input and snapshot fold_generation from BEFORE that mutation. Do
        // NOT arm step_batch_remaining here — see the ordering-guarantee doc
        // above for why that has to wait for a confirmed fold.
        let (fold_start, masks_given) = match self.debug.lock() {
            Ok(mut ds) => {
                if !ds.paused {
                    return json!({
                        "ok": false,
                        "error": "run_frames requires the emulator to be paused first (pause, \
                                  then run_frames) — same precondition as the pause→step \
                                  frame-exact workflow",
                    });
                }
                let fold_start = ds.fold_generation;
                let mut masks_given = false;
                if let Some(bits) = bits0 {
                    ds.set_held_input(0, bits);
                    masks_given = true;
                }
                if let Some(bits) = bits1 {
                    ds.set_held_input(1, bits);
                    masks_given = true;
                }
                (fold_start, masks_given)
            }
            Err(_) => return json!({ "ok": false, "error": "lock poisoned" }),
        };

        // Phase 1.5: only when masks were actually set, block until the host
        // loop's next fold has observed them — the fix for the race. The
        // no-mask path (measured clean at 0/200) skips this entirely.
        if masks_given {
            match self.wait_for_fold(fold_start, FOLD_WAIT_TIMEOUT) {
                Ok(Some(_)) => {}
                Ok(None) => {
                    return json!({
                        "ok": false,
                        "error": format!(
                            "timed out after {:?} waiting for the host loop to fold the new \
                             port0/port1 masks — the masks ARE set (held) but no batch was \
                             armed, so nothing ran on a possibly-stale fold; the host loop may \
                             be wedged",
                            FOLD_WAIT_TIMEOUT,
                        ),
                    });
                }
                Err(e) => return json!({ "ok": false, "error": e }),
            }
        }

        // Phase 2: arm the batch — a SEPARATE lock acquisition from phase 1,
        // now that (for the masked path) a fold of the new masks is
        // confirmed. Re-checks `paused` since it may have changed while we
        // waited.
        let (start_gen, start_frame) = match self.debug.lock() {
            Ok(mut ds) => {
                if !ds.paused {
                    return json!({
                        "ok": false,
                        "error": "run_frames requires the emulator to be paused first — it was \
                                  paused when the masks were set but was resumed before the \
                                  batch could be armed; the masks are still held, retry",
                    });
                }
                let start_gen = ds.step_generation;
                let start_frame = ds.frame_count;
                ds.step_batch_remaining = count;
                (start_gen, start_frame)
            }
            Err(_) => return json!({ "ok": false, "error": "lock poisoned" }),
        };

        let timeout = RUN_FRAMES_TIMEOUT_FLOOR + RUN_FRAMES_PER_FRAME_TIMEOUT * count;
        let (landed, end_frame, timed_out) = match self.wait_for_batch(start_gen, count, timeout) {
            Ok(v) => v,
            Err(e) => return json!({ "ok": false, "error": e }),
        };

        json!({
            "ok": !timed_out,
            "start_frame": start_frame,
            "end_frame": end_frame,
            "requested": count,
            "landed": landed,
            "all_landed": landed == count as u64,
            "error": if timed_out {
                json!(format!(
                    "timed out after {:?} waiting for {} of {} requested frames — the \
                     remaining auto-step budget was cleared",
                    timeout, count as u64 - landed, count,
                ))
            } else {
                Value::Null
            },
        })
    }

    /// Submit a Lua script to the main thread and poll for its result.
    fn run_lua(&self, script: String) -> Value {
        // Submit.
        {
            let mut ds = match self.debug.lock() {
                Ok(g) => g,
                Err(_) => return json!({ "ok": false, "error": "lock poisoned" }),
            };
            if ds.pending_lua.is_some() {
                return json!({ "ok": false, "error": "another Lua request is in flight" });
            }
            ds.pending_lua_result = None;
            ds.pending_lua = Some(script);
        }

        // Poll for completion. The drain system runs once per Bevy Update frame
        // (~60Hz), so this normally resolves within a frame or two.
        let deadline = Instant::now() + LUA_TIMEOUT;
        loop {
            std::thread::sleep(Duration::from_millis(8));
            if let Ok(mut ds) = self.debug.lock() {
                if let Some(res) = ds.pending_lua_result.take() {
                    return match res {
                        Ok(out) => json!({ "ok": true, "output": out }),
                        Err(e) => json!({ "ok": false, "error": e }),
                    };
                }
            }
            if Instant::now() >= deadline {
                // Clear our request so we don't wedge future calls.
                if let Ok(mut ds) = self.debug.lock() {
                    ds.pending_lua = None;
                }
                return json!({
                    "ok": false,
                    "error": "timed out waiting for main thread (is the app running?)"
                });
            }
        }
    }

    // ── save states ────────────────────────────────────────────────────────

    /// Submit a [`StateOp`] to the emulation thread and poll for its result —
    /// the same deferred round-trip as `run_lua` (core FFI only happens on the
    /// emu thread; `Frontend::drain_state_op` services the queue every frame,
    /// paused or not).
    fn state_op_roundtrip(&self, op: StateOp) -> Value {
        // Submit.
        {
            let mut ds = match self.debug.lock() {
                Ok(g) => g,
                Err(_) => return json!({ "ok": false, "error": "lock poisoned" }),
            };
            if ds.pending_state_op.is_some() {
                return json!({ "ok": false, "error": "another state operation is in flight" });
            }
            ds.state_op_result = None;
            ds.pending_state_op = Some(op);
        }

        // Poll for completion (drained once per emulated frame, ~60Hz).
        let deadline = Instant::now() + STATE_TIMEOUT;
        loop {
            std::thread::sleep(Duration::from_millis(8));
            if let Ok(mut ds) = self.debug.lock() {
                if let Some(res) = ds.state_op_result.take() {
                    return match res {
                        Ok(done) => json!({
                            "ok": true,
                            "op": if done.loaded { "load" } else { "save" },
                            "path": done.path.display().to_string(),
                            "bytes": done.bytes,
                        }),
                        Err(e) => json!({ "ok": false, "error": e }),
                    };
                }
            }
            if Instant::now() >= deadline {
                // Clear our request so we don't wedge future calls.
                if let Ok(mut ds) = self.debug.lock() {
                    ds.pending_state_op = None;
                }
                return json!({
                    "ok": false,
                    "error": "timed out waiting for the emulation thread (is the app running?)"
                });
            }
        }
    }

    /// `save_state`: serialize the core to a slot file or explicit path.
    /// NOT gated — it reads game state and writes only a state file.
    fn save_state(&self, slot: Option<u64>, path: Option<&str>) -> Value {
        match parse_state_target(slot, path, false) {
            Ok(op) => self.state_op_roundtrip(op),
            Err(e) => json!({ "ok": false, "error": e }),
        }
    }

    /// `load_state`: restore the core from a slot file or explicit path.
    /// GATED — it replaces the entire game state.
    fn load_state(&self, slot: Option<u64>, path: Option<&str>) -> Value {
        if let Err(e) = self.check_writes_armed() {
            return json!({ "error": e });
        }
        match parse_state_target(slot, path, true) {
            Ok(op) => self.state_op_roundtrip(op),
            Err(e) => json!({ "ok": false, "error": e }),
        }
    }

    /// `load_shadow`: (re)load a shadow-bot model directory at runtime (the
    /// scripted twin of the Training panel's model picker — lets loop.sh push
    /// a freshly fitted model into a running session). Same roundtrip shape
    /// as `state_op_roundtrip` over `pending_shadow_load`/`shadow_load_result`.
    /// The model arrives DISABLED unless a shadow was already enabled (a swap
    /// keeps fighting); enabling stays with Shift+F5 / the Training panel.
    /// GATED — an already-active shadow's behavior changes immediately.
    fn load_shadow(&self, path: &str) -> Value {
        if let Err(e) = self.check_writes_armed() {
            return json!({ "error": e });
        }
        {
            let mut ds = match self.debug.lock() {
                Ok(g) => g,
                Err(_) => return json!({ "ok": false, "error": "lock poisoned" }),
            };
            if ds.pending_shadow_load.is_some() {
                return json!({ "ok": false, "error": "another shadow load is in flight" });
            }
            ds.shadow_load_result = None;
            ds.pending_shadow_load = Some(std::path::PathBuf::from(path));
        }
        let deadline = Instant::now() + STATE_TIMEOUT;
        loop {
            std::thread::sleep(Duration::from_millis(8));
            if let Ok(mut ds) = self.debug.lock() {
                if let Some(res) = ds.shadow_load_result.take() {
                    return match res {
                        Ok(msg) => json!({ "ok": true, "message": msg }),
                        Err(e) => json!({ "ok": false, "error": e }),
                    };
                }
            }
            if Instant::now() >= deadline {
                if let Ok(mut ds) = self.debug.lock() {
                    ds.pending_shadow_load = None;
                }
                return json!({
                    "ok": false,
                    "error": "timed out waiting for the emulation thread (is the app running?)"
                });
            }
        }
    }

    // ── gated write tools ──────────────────────────────────────────────────

    /// `write_memory`: poke `len` little-endian bytes of `value` at guest `addr`
    /// via the bounds-checked [`DebugState::write_addr`]. GATED: refuses unless
    /// writes are armed. Returns an error (without writing) if `write_addr`
    /// reports the target is read-only or unbacked.
    fn write_memory(&self, addr: usize, len: usize, value: u32) -> Value {
        if let Err(e) = self.check_writes_armed() {
            return json!({ "error": e });
        }
        let len = len.clamp(1, 4);
        let mut ds = match self.debug.lock() {
            Ok(g) => g,
            Err(_) => return json!({ "error": "debug state lock poisoned" }),
        };
        let region = ds
            .memory_regions
            .iter()
            .find(|r| addr >= r.addr_start && addr <= r.addr_end)
            .map(|r| r.name.clone());
        let wrote = ds.write_addr(addr, len, value);
        drop(ds);
        if !wrote {
            return json!({
                "ok": false,
                "addr": format!("0x{addr:X}"),
                "region": region,
                "error": "write refused: address is read-only or not backed by writable memory",
            });
        }
        json!({
            "ok": true,
            "wrote": true,
            "addr": format!("0x{addr:X}"),
            "len": len,
            "value": value,
            "region": region,
        })
    }

    /// `freeze`: add (or update) a frozen [`Watch`] at `addr`. With `value`, freeze
    /// to that value; otherwise capture the current value. This matches the UI
    /// freeze exactly: the run loop re-writes every watch with `frozen == true`
    /// each frame, using `frozen_value` (capturing the current value when it is
    /// `None`). GATED.
    fn freeze(&self, addr: usize, format: WatchFormat, value: Option<u32>) -> Value {
        if let Err(e) = self.check_writes_armed() {
            return json!({ "error": e });
        }
        let mut ds = match self.debug.lock() {
            Ok(g) => g,
            Err(_) => return json!({ "error": "debug state lock poisoned" }),
        };
        // Determine the value to hold: explicit, else the current memory value.
        let frozen_value = match value {
            Some(v) => Some(v),
            None => ds.read_addr(addr, format.byte_len()),
        };
        // Update an existing watch at this addr, or append a new one.
        if let Some(w) = ds.watches.iter_mut().find(|w| w.addr == addr) {
            w.format = format;
            w.frozen = true;
            w.frozen_value = frozen_value;
        } else {
            ds.watches.push(Watch {
                addr,
                label: format!("{addr:06X}"),
                format,
                frozen: true,
                frozen_value,
                track_changes: false,
                current: None,
                prev_value: None,
            });
        }
        let watch = ds.watches.iter().find(|w| w.addr == addr).cloned();
        drop(ds);
        json!({ "ok": true, "watch": watch })
    }

    /// `unfreeze`: clear the freeze on the watch at `addr` (leaving the watch in
    /// place, like un-checking the UI freeze box, which also clears
    /// `frozen_value`). GATED. Returns whether a matching watch was found.
    fn unfreeze(&self, addr: usize) -> Value {
        if let Err(e) = self.check_writes_armed() {
            return json!({ "error": e });
        }
        let mut ds = match self.debug.lock() {
            Ok(g) => g,
            Err(_) => return json!({ "error": "debug state lock poisoned" }),
        };
        let found = if let Some(w) = ds.watches.iter_mut().find(|w| w.addr == addr) {
            w.frozen = false;
            w.frozen_value = None;
            true
        } else {
            false
        };
        drop(ds);
        json!({ "ok": found, "addr": format!("0x{addr:X}"), "unfrozen": found })
    }

    /// `set_breakpoint`: add an M68K PC breakpoint (deduped, capped at
    /// [`MAX_BREAKPOINTS`] to match the UI). GATED.
    fn set_breakpoint(&self, addr: u32) -> Value {
        if let Err(e) = self.check_writes_armed() {
            return json!({ "error": e });
        }
        let mut ds = match self.debug.lock() {
            Ok(g) => g,
            Err(_) => return json!({ "error": "debug state lock poisoned" }),
        };
        if ds.breakpoints.contains(&addr) {
            let list = ds.breakpoints.clone();
            drop(ds);
            return json!({ "ok": true, "added": false, "reason": "already set",
                           "addr": format!("0x{addr:X}"), "breakpoints": fmt_addrs(&list) });
        }
        if ds.breakpoints.len() >= MAX_BREAKPOINTS {
            let list = ds.breakpoints.clone();
            drop(ds);
            return json!({ "ok": false, "added": false,
                           "error": format!("breakpoint limit reached (max {MAX_BREAKPOINTS})"),
                           "breakpoints": fmt_addrs(&list) });
        }
        ds.breakpoints.push(addr);
        let list = ds.breakpoints.clone();
        drop(ds);
        json!({ "ok": true, "added": true, "addr": format!("0x{addr:X}"),
                "breakpoints": fmt_addrs(&list) })
    }

    /// `clear_breakpoint`: remove an M68K PC breakpoint. GATED.
    fn clear_breakpoint(&self, addr: u32) -> Value {
        if let Err(e) = self.check_writes_armed() {
            return json!({ "error": e });
        }
        let mut ds = match self.debug.lock() {
            Ok(g) => g,
            Err(_) => return json!({ "error": "debug state lock poisoned" }),
        };
        let before = ds.breakpoints.len();
        ds.breakpoints.retain(|&a| a != addr);
        let removed = ds.breakpoints.len() != before;
        let list = ds.breakpoints.clone();
        drop(ds);
        json!({ "ok": true, "removed": removed, "addr": format!("0x{addr:X}"),
                "breakpoints": fmt_addrs(&list) })
    }

    /// `list_breakpoints`: report the current M68K PC breakpoints. Ungated (read).
    fn list_breakpoints(&self) -> Value {
        let list = match self.debug.lock() {
            Ok(ds) => ds.breakpoints.clone(),
            Err(_) => return json!({ "error": "debug state lock poisoned" }),
        };
        json!({ "breakpoints": fmt_addrs(&list), "count": list.len() })
    }

    /// `run_to`: arm a one-shot run-to-address; the run loop pauses when the M68K
    /// PC reaches `addr`. GATED (it changes execution flow).
    fn run_to(&self, addr: u32) -> Value {
        if let Err(e) = self.check_writes_armed() {
            return json!({ "error": e });
        }
        let mut ds = match self.debug.lock() {
            Ok(g) => g,
            Err(_) => return json!({ "error": "debug state lock poisoned" }),
        };
        ds.run_to_addr = Some(addr);
        drop(ds);
        json!({ "ok": true, "run_to_addr": format!("0x{addr:X}") })
    }

    // ── ROM-map writeback (AI-authored region persistence) ──────────────────

    /// `get_rom_map`: return the current literate ROM-map Markdown (read-only,
    /// UNGATED) so the agent can review what's already recorded before adding to
    /// it. Reports `exists: false` with the resolved path when no map exists yet.
    fn get_rom_map(&self) -> Value {
        let path = {
            let ds = match self.debug.lock() {
                Ok(g) => g,
                Err(_) => return json!({ "error": "debug state lock poisoned" }),
            };
            ds.rom_map_path.clone()
        };
        let path = match path {
            Some(p) => p,
            None => {
                return json!({
                    "error": "no ROM map path (ROM not loaded with a library path)"
                })
            }
        };
        match std::fs::read_to_string(&path) {
            Ok(md) => json!({
                "ok": true,
                "exists": true,
                "path": path.display().to_string(),
                "markdown": md,
            }),
            Err(_) => json!({
                "ok": true,
                "exists": false,
                "path": path.display().to_string(),
                "markdown": Value::Null,
                "note": "no map yet — add_rom_map_region will scaffold one",
            }),
        }
    }

    /// `add_rom_map_region`: persist a confirmed RE finding into the ROM's
    /// literate Markdown map as an `author=ai` `::: region` block. GATED (it
    /// mutates a file). Validates `kind`/`confidence` against the §5/§4 vocab,
    /// scaffolds the map if missing (§9), assigns a collision-free `ai<n>` id,
    /// appends the block into `## Regions` (never touching existing prose, §6),
    /// and writes atomically (`.tmp` + rename).
    fn add_rom_map_region(
        &self,
        kind: &str,
        addr: &str,
        label: Option<&str>,
        confidence: Option<&str>,
        note: Option<&str>,
    ) -> Value {
        if let Err(e) = self.check_writes_armed() {
            return json!({ "error": e });
        }

        // Validate kind against the controlled vocabulary (§5).
        if !ROM_MAP_KINDS.contains(&kind) {
            return json!({
                "error": format!("unknown kind '{kind}'"),
                "valid_kinds": ROM_MAP_KINDS,
            });
        }

        // Validate / default confidence (§4).
        let confidence = confidence.unwrap_or("likely");
        if !ROM_MAP_CONFIDENCES.contains(&confidence) {
            return json!({
                "error": format!("unknown confidence '{confidence}'"),
                "valid_confidence": ROM_MAP_CONFIDENCES,
            });
        }

        // Normalize/validate the addr token: "0xSTART-0xEND" or a single "0xADDR".
        let addr = match normalize_addr(addr) {
            Ok(a) => a,
            Err(e) => return json!({ "error": e }),
        };

        let note = note.unwrap_or(DEFAULT_REGION_NOTE);

        // Pull the map path + identity fields under a brief lock.
        let (path, rom_name, rom_sha1, rom_size, rom_system) = {
            let ds = match self.debug.lock() {
                Ok(g) => g,
                Err(_) => return json!({ "error": "debug state lock poisoned" }),
            };
            (
                ds.rom_map_path.clone(),
                ds.rom_name.clone(),
                ds.rom_sha1.clone(),
                ds.rom_size,
                ds.rom_system.clone(),
            )
        };
        let path = match path {
            Some(p) => p,
            None => {
                return json!({
                    "error": "no ROM map path (ROM not loaded with a library path)"
                })
            }
        };

        // Read the existing map, or scaffold a fresh one (§9) if absent. We never
        // create the parent dir lazily inside the helper — do it here so a write
        // error surfaces cleanly.
        let existing = match std::fs::read_to_string(&path) {
            Ok(md) => md,
            Err(_) => {
                if let Some(parent) = path.parent() {
                    if let Err(e) = std::fs::create_dir_all(parent) {
                        return json!({
                            "error": format!("failed to create library dir: {e}")
                        });
                    }
                }
                scaffold_rom_map(
                    rom_name.as_deref(),
                    rom_sha1.as_deref(),
                    rom_size,
                    rom_system.as_deref(),
                )
            }
        };

        // Assign a collision-free AI id and build the new content.
        let id = next_ai_id(&existing);
        let new_md =
            append_region_block(&existing, &id, kind, &addr, label, confidence, "ai", note);

        // Atomic write: <path>.tmp then rename over the original (§6).
        let tmp = path.with_extension("md.tmp");
        if let Err(e) = std::fs::write(&tmp, &new_md) {
            return json!({ "error": format!("failed to write tmp map: {e}") });
        }
        if let Err(e) = std::fs::rename(&tmp, &path) {
            let _ = std::fs::remove_file(&tmp);
            return json!({ "error": format!("failed to rename map into place: {e}") });
        }

        json!({
            "ok": true,
            "id": id,
            "path": path.display().to_string(),
            "kind": kind,
            "addr": addr,
            "confidence": confidence,
            "author": "ai",
        })
    }

    // ── tool catalog ───────────────────────────────────────────────────────

    /// Build the static tool list advertised to clients.
    fn tools() -> Vec<Tool> {
        // An empty-object schema (no required params).
        let no_params = || -> Arc<Map<String, Value>> {
            let mut m = Map::new();
            m.insert("type".into(), json!("object"));
            m.insert("properties".into(), json!({}));
            Arc::new(m)
        };
        // Schema for { addr, len }.
        let read_memory_schema = || -> Arc<Map<String, Value>> {
            let schema = json!({
                "type": "object",
                "properties": {
                    "addr": { "type": "integer", "description": "Guest address (decimal or use 0x via JSON number)" },
                    "len":  { "type": "integer", "description": "Number of bytes to read (max 4096)" }
                },
                "required": ["addr", "len"]
            });
            Arc::new(schema.as_object().unwrap().clone())
        };
        // Schema for { region_name, offset, len }.
        let read_region_schema = || -> Arc<Map<String, Value>> {
            let schema = json!({
                "type": "object",
                "properties": {
                    "region_name": { "type": "string", "description": "Exact region name (see app://memory-map / list_regions), e.g. \"VRAM\", \"ROM\"" },
                    "offset": { "type": "integer", "description": "Byte offset WITHIN the region (default 0)" },
                    "len":    { "type": "integer", "description": "Number of bytes to read (max 8192)" }
                },
                "required": ["region_name", "len"]
            });
            Arc::new(schema.as_object().unwrap().clone())
        };
        // Schema for { needle_hex, scope }.
        let search_memory_schema = || -> Arc<Map<String, Value>> {
            let schema = json!({
                "type": "object",
                "properties": {
                    "needle_hex": { "type": "string", "description": "Hex byte pattern to find, e.g. \"DEADBEEF\" or \"DE AD BE EF\" (4..256 bytes)" },
                    "scope":      { "type": "string", "description": "Region name to scan, or \"all\" / omitted to scan every region" }
                },
                "required": ["needle_hex"]
            });
            Arc::new(schema.as_object().unwrap().clone())
        };
        // Schema for { vram_addr, len }.
        let vram_to_rom_schema = || -> Arc<Map<String, Value>> {
            let schema = json!({
                "type": "object",
                "properties": {
                    "vram_addr": { "type": "integer", "description": "Absolute guest address inside a VRAM/RAM region holding the tile bytes" },
                    "len":       { "type": "integer", "description": "Number of bytes to lift and search for in ROM (4..256, default 32)" }
                },
                "required": ["vram_addr"]
            });
            Arc::new(schema.as_object().unwrap().clone())
        };
        // Schema for { source, offset?, len?, format, tiles_per_row? } (render_tiles).
        let render_tiles_schema = || -> Arc<Map<String, Value>> {
            let schema = json!({
                "type": "object",
                "properties": {
                    "source": { "type": "string", "description": "Where to read bytes: an exact region NAME (see app://memory-map); a convenience \"rom\" / \"vram\" / \"memory\" (first ROM/VRAM/any backed region); \"rom_file\" — the whole on-disk CART file (content the core hides); or a NES iNES span \"rom_file:chr\" / \"rom_file:prg\" / \"rom_file:header\" (call rom_info for the layout). For NES graphics use source=rom_file:chr with format=nes_chr — offset 0 then lands on the first CHR tile." },
                    "offset": { "type": "integer", "description": "Byte offset WITHIN the source region/file (default 0)" },
                    "len":    { "type": "integer", "description": "Number of bytes to decode (capped at 65536)" },
                    "format": { "type": "string", "description": "Tile pixel format: 2bpp | nes_chr (NES, 16 B/tile, 4 colors) or 4bpp | genesis (Genesis, 32 B/tile, 16 colors)" },
                    "tiles_per_row": { "type": "integer", "description": "Tiles laid out per row in the image grid (default 16, max 64)" }
                },
                "required": ["source", "format"]
            });
            Arc::new(schema.as_object().unwrap().clone())
        };
        // Schema for { name, addr, len, interval? } (map_bus_window).
        let map_bus_window_schema = || -> Arc<Map<String, Value>> {
            let schema = json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Region name to publish (must not collide with an existing region), e.g. \"Sprite RAM\"" },
                    "addr": { "type": ["integer", "string"], "description": "Guest bus address of the window start — integer or hex string \"0x600000\"" },
                    "len":  { "type": ["integer", "string"], "description": "Window length in bytes (max 1 MiB) — integer or hex string" },
                    "interval": { "type": "integer", "description": "Refresh every N frames (default 1 = every frame)" }
                },
                "required": ["name", "addr", "len"]
            });
            Arc::new(schema.as_object().unwrap().clone())
        };
        // Schema for { source, window? } (scan_regions).
        let scan_regions_schema = || -> Arc<Map<String, Value>> {
            let schema = json!({
                "type": "object",
                "properties": {
                    "source": { "type": "string", "description": "What to scan: an exact region NAME (see app://memory-map); a convenience \"rom\" / \"vram\" / \"memory\" (first ROM/VRAM/any backed region); \"rom_file\" — the whole on-disk CART (scans content the core hides, e.g. NES PRG+CHR); or a NES iNES span \"rom_file:prg\" / \"rom_file:chr\". Default \"rom\"." },
                    "window": { "type": "integer", "description": "Sampling window in bytes (default 4096, min 256, max 65536). Smaller = finer boundaries, more candidates." }
                },
                "required": []
            });
            Arc::new(schema.as_object().unwrap().clone())
        };
        let run_lua_schema = || -> Arc<Map<String, Value>> {
            let schema = json!({
                "type": "object",
                "properties": {
                    "script": { "type": "string", "description": "Lua source to execute on the main thread" }
                },
                "required": ["script"]
            });
            Arc::new(schema.as_object().unwrap().clone())
        };
        // Schema for { buttons:[string], frames? } (press_buttons).
        let press_buttons_schema = || -> Arc<Map<String, Value>> {
            let schema = json!({
                "type": "object",
                "properties": {
                    "buttons": { "type": "array", "items": { "type": "string" }, "description": "Buttons to hold simultaneously: a, b, x, y, l, r, start, select, up, down, left, right" },
                    "frames": { "type": "integer", "description": "How many emulated frames to hold (default 8, max 600 ≈ 10s at 60fps)" },
                    "port": { "type": "integer", "description": "Controller port: 0 = P1 (default), 1 = P2 (the second fighter / dummy slot)" }
                },
                "required": ["buttons"]
            });
            Arc::new(schema.as_object().unwrap().clone())
        };
        // Schema for { buttons:[string], port? } (hold_buttons).
        let hold_buttons_schema = || -> Arc<Map<String, Value>> {
            let schema = json!({
                "type": "object",
                "properties": {
                    "buttons": { "type": "array", "items": { "type": "string" }, "description": "Buttons to hold simultaneously (a, b, x, y, l, r, start, select, up, down, left, right). REPLACES the port's whole held set; [] releases everything." },
                    "port": { "type": "integer", "description": "Controller port: 0 = P1 (default), 1 = P2 (the second fighter / dummy slot)" }
                },
                "required": ["buttons"]
            });
            Arc::new(schema.as_object().unwrap().clone())
        };
        // Schema for { count, port0?:[string], port1?:[string] } (run_frames).
        let run_frames_schema = || -> Arc<Map<String, Value>> {
            let schema = json!({
                "type": "object",
                "properties": {
                    "count": { "type": "integer", "description": "Frames to advance synchronously (1-600 ≈ 10s at 60fps)" },
                    "port0": { "type": "array", "items": { "type": "string" }, "description": "Buttons to HOLD on P1 for the whole run (REPLACES P1's held set, same semantics as hold_buttons); omit to leave P1's current held set unchanged. When given, run_frames blocks until the host loop has confirmed folding this mask before counting frame 1 — the first counted frame is guaranteed to see it, not a stale prior fold." },
                    "port1": { "type": "array", "items": { "type": "string" }, "description": "Same as port0 but for P2." }
                },
                "required": ["count"]
            });
            Arc::new(schema.as_object().unwrap().clone())
        };
        // Schema for { buttons?:[string], port? } (release_buttons).
        let release_buttons_schema = || -> Arc<Map<String, Value>> {
            let schema = json!({
                "type": "object",
                "properties": {
                    "buttons": { "type": "array", "items": { "type": "string" }, "description": "Buttons to release; omit (or []) to release the whole port's held set" },
                    "port": { "type": "integer", "description": "Controller port: 0 = P1 (default), 1 = P2 (the second fighter / dummy slot)" }
                },
                "required": []
            });
            Arc::new(schema.as_object().unwrap().clone())
        };
        // Schema for { port? } (get_input).
        let get_input_schema = || -> Arc<Map<String, Value>> {
            let schema = json!({
                "type": "object",
                "properties": {
                    "port": { "type": "integer", "description": "Controller port: 0 = P1 (default), 1 = P2 (the second fighter / dummy slot)" }
                },
                "required": []
            });
            Arc::new(schema.as_object().unwrap().clone())
        };
        // Schema for { action, name? } (record_inputs).
        let record_inputs_schema = || -> Arc<Map<String, Value>> {
            let schema = json!({
                "type": "object",
                "properties": {
                    "action": { "type": "string", "description": "\"start\" or \"stop\"" },
                    "name": { "type": "string", "description": "Slot name (required for action=\"start\"); becomes shadow/inputs/<family>/<name>.slot.json" }
                },
                "required": ["action"]
            });
            Arc::new(schema.as_object().unwrap().clone())
        };
        // Schema for { action, name?, port?, trigger? } (play_inputs).
        let play_inputs_schema = || -> Arc<Map<String, Value>> {
            let schema = json!({
                "type": "object",
                "properties": {
                    "action": { "type": "string", "description": "\"start\" or \"stop\"" },
                    "name": { "type": "string", "description": "Slot name to play (required for action=\"start\")" },
                    "port": { "type": "string", "description": "\"p1\", \"p2\", or \"both\" (default \"both\")" },
                    "trigger": { "type": "string", "description": "\"manual\" (begin on the next real frame — pair with pause/step for frame-exact timing) or \"round_start\" (begin on the fight gate's next closed→open transition; deterministic from a pre-round save state). Default \"manual\"." }
                },
                "required": ["action"]
            });
            Arc::new(schema.as_object().unwrap().clone())
        };
        // ── signal hunt (docs/signal-hunt.md §8) ───────────────────────────
        let hunt_mark_schema = || -> Arc<Map<String, Value>> {
            let schema = json!({
                "type": "object",
                "properties": {
                    "label": { "type": "string", "description": "Free-form label. Convention: \"event\" for the thing you are hunting, \"control\" for a near-miss that must NOT produce the signal. Multiple labels may be in play." }
                },
                "required": ["label"]
            });
            Arc::new(schema.as_object().unwrap().clone())
        };
        let hunt_configure_schema = || -> Arc<Map<String, Value>> {
            let schema = json!({
                "type": "object",
                "properties": {
                    "blocks": { "type": "boolean", "description": "Include both fighter structs (profile block1/block2 + stride). Default true — this is the §3 default scope." },
                    "start":  { "type": ["integer", "string"], "description": "Extra window start (guest address; integer or hex string \"0xBC00\"). Use for values OUTSIDE the fighter structs, e.g. MK2's HUD health pair." },
                    "len":    { "type": ["integer", "string"], "description": "Extra window length in bytes. Required with `start`." },
                    "ring_frames": { "type": "integer", "description": "Snapshots retained in the rolling ring (default 60, min 2)." },
                    "pre":  { "type": "integer", "description": "Frames BEFORE a mark forming the 'before' side of its changed-set (default 4)." },
                    "post": { "type": "integer", "description": "Frames AFTER a mark forming the 'after' side (default 12)." },
                    "include_idle": { "type": "boolean", "description": "Subtract the idle-churn set from candidates (default true). The analysis reports the result BOTH ways regardless." },
                    "enabled": { "type": "boolean", "description": "Turn per-frame sampling on/off (default on — the ring must already be running before the first mark)." }
                },
                "required": []
            });
            Arc::new(schema.as_object().unwrap().clone())
        };
        let hunt_analyze_schema = || -> Arc<Map<String, Value>> {
            let schema = json!({
                "type": "object",
                "properties": {
                    "event_label":   { "type": "string", "description": "Label of the marks that ARE the event (default \"event\")." },
                    "control_label": { "type": "string", "description": "Label of the near-miss marks. OMITTING THIS IS DANGEROUS — the report will warn prominently, because a hunt with no control is how a swing counter was mistaken for a contact signal." }
                },
                "required": []
            });
            Arc::new(schema.as_object().unwrap().clone())
        };

        // Schema for { addr, len, value } (write_memory).
        let write_memory_schema = || -> Arc<Map<String, Value>> {
            let schema = json!({
                "type": "object",
                "properties": {
                    "addr":  { "type": "integer", "description": "Guest address to write" },
                    "len":   { "type": "integer", "description": "Number of little-endian bytes (1..=4)" },
                    "value": { "type": "integer", "description": "Value to write (little-endian, low `len` bytes used)" }
                },
                "required": ["addr", "len", "value"]
            });
            Arc::new(schema.as_object().unwrap().clone())
        };
        // Schema for { addr, format, value? } (freeze).
        let freeze_schema = || -> Arc<Map<String, Value>> {
            let schema = json!({
                "type": "object",
                "properties": {
                    "addr":   { "type": "integer", "description": "Guest address to freeze" },
                    "format": { "type": "string", "description": "Watch format: u8, s8, u16_le, u16_be, u32_le, u32_be, hex8, hex16, hex32" },
                    "value":  { "type": "integer", "description": "Optional value to freeze to; if omitted, the current value is captured" }
                },
                "required": ["addr", "format"]
            });
            Arc::new(schema.as_object().unwrap().clone())
        };
        // Schema for { kind, addr, label?, confidence?, note? } (add_rom_map_region).
        let add_rom_map_region_schema = || -> Arc<Map<String, Value>> {
            let schema = json!({
                "type": "object",
                "properties": {
                    "kind": { "type": "string", "description": "Controlled vocab (ROM_MAP_FORMAT §5): game_loop, subroutine, interrupt_handler, sound_driver, title_screen, background, tilemap, character_sprite, sprite_sheet, palette, music_track, sfx_table, level_data, text_table, lookup_table" },
                    "addr": { "type": "string", "description": "Address: a single point \"0xADDR\" or a range \"0xSTART-0xEND\"" },
                    "label": { "type": "string", "description": "Optional short human name for the region" },
                    "confidence": { "type": "string", "description": "confirmed | likely | guess (default likely)" },
                    "note": { "type": "string", "description": "Optional prose stub line (the human-owned zone); defaults to a generic note" }
                },
                "required": ["kind", "addr"]
            });
            Arc::new(schema.as_object().unwrap().clone())
        };
        // Schema for { slot?, path? } (save_state / load_state).
        let state_target_schema = || -> Arc<Map<String, Value>> {
            let schema = json!({
                "type": "object",
                "properties": {
                    "slot": { "type": "integer", "description": "Save-state slot 1-9 → <save_dir>/<rom>.state<N> (default 1 when `path` is absent)" },
                    "path": { "type": "string", "description": "Explicit state-file path (mutually exclusive with `slot`)" }
                },
                "required": []
            });
            Arc::new(schema.as_object().unwrap().clone())
        };
        // Schema for { addr } (unfreeze / breakpoint ops / run_to).
        let addr_only_schema = || -> Arc<Map<String, Value>> {
            let schema = json!({
                "type": "object",
                "properties": {
                    "addr": { "type": "integer", "description": "Guest address" }
                },
                "required": ["addr"]
            });
            Arc::new(schema.as_object().unwrap().clone())
        };

        vec![
            Tool::new(
                "get_state",
                "Return a JSON snapshot of the live app (frame, fps, AV/FB dims, M68K+Z80 registers, paused, memory-region summaries, collection counts, nav address).",
                no_params(),
            ),
            Tool::new(
                "read_memory",
                "Read up to 4096 bytes from a guest address. Returns hex bytes and the containing region name.",
                read_memory_schema(),
            ),
            Tool::new(
                "list_regions",
                "List the mapped memory regions (name, type, address range, size, readonly).",
                no_params(),
            ),
            Tool::new(
                "map_bus_window",
                "Publish a live memory region for cores with NO libretro memory map by \
                 snapshotting a window of the emulated CPU bus each frame through the core's \
                 exported bus-read API (fbalpha2012 exports its 68k `SekRead*`; the window \
                 then feeds read_memory / read_region / search_memory / render_tiles / Lua \
                 like any region). Give the window a name, a guest bus address, and a length. \
                 ONLY map RAM-backed ranges (work RAM, VRAM, sprite/palette RAM — e.g. from \
                 the game's MAME driver memory map); reading I/O handler ranges can perturb \
                 the machine. Persists to the busmap sidecar. If the core lacks the API the \
                 window stays zero-filled and the event log says so.",
                map_bus_window_schema(),
            ),
            Tool::new(
                "read_region",
                "Read up to 8192 bytes from WITHIN a named memory region at a byte offset. \
                 Lets you inspect VRAM, object/sprite RAM, or ROM by NAME without knowing \
                 absolute guest addresses. Returns hex bytes, the region kind, and the \
                 resolved absolute address.",
                read_region_schema(),
            ),
            Tool::new(
                "search_memory",
                "Scan one named region (or all regions) for a hex byte pattern and return the \
                 absolute addresses where it occurs. This is a CONTENT match, NOT a DMA \
                 transfer trace: it finds where identical bytes live, which is the achievable \
                 substitute for true VRAM→ROM provenance (the loaded cores expose no DMA \
                 source→dest hook). Needle 4..256 bytes; results capped.",
                search_memory_schema(),
            ),
            Tool::new(
                "vram_to_rom",
                "Convenience 'where did this tile come from' primitive: read `len` bytes of \
                 VRAM at `vram_addr`, then content-search every ROM-type region for that exact \
                 block, returning candidate ROM addresses + region names. HONEST CAVEAT: this \
                 is a content match, NOT DMA-traced provenance — it can return false positives \
                 (coincidental byte matches) or NOTHING if the ROM stores the graphics \
                 compressed or in a different bitplane/tile layout. Corroborate with longer \
                 blocks.",
                vram_to_rom_schema(),
            ),
            Tool::new(
                "render_tiles",
                "IMAGE-RECOGNITION RE primitive: decode a span of ROM/VRAM bytes AS tiles and \
                 return it as a PNG IMAGE so you can SEE it and visually identify graphics — \
                 e.g. compare a candidate ROM region to the sprite/character on app://screen. \
                 This is the visual evidence stream that COMPLEMENTS vram_to_rom (a raw \
                 byte-content match): rendering survives compressed / re-bitplaned graphics \
                 where verbatim hex matching fails, because it judges PIXELS not bytes. Use \
                 both for convergent evidence. `source` is a region NAME, rom/vram/memory, or \
                 \"rom_file\" (the on-disk CART — decodes graphics the core HIDES, e.g. NES \
                 CHR-ROM at its iNES file offset); \
                 `format` is 2bpp|nes_chr (NES, 16 B/tile) or 4bpp|genesis (Genesis, 32 B/tile, \
                 generic 4bpp planar — approximate for CPS2). Palette is unknown by default, so \
                 a grayscale ramp is used to expose structure. Read-only (no enable_writes).",
                render_tiles_schema(),
            ),
            Tool::new(
                "scan_regions",
                "STRUCTURE RE primitive: window a region's bytes and propose what KIND each span \
                 looks like (padding / text_table / packed_data / lookup_table / graphics / code) \
                 from cheap statistical signatures — Shannon entropy, byte-histogram spikiness, \
                 printable/fill fractions. Use this to ORIENT inside an unknown ROM ('this 512 KB \
                 span looks like packed sprite data') BEFORE zeroing in with the precise streams. \
                 Returns coalesced candidate spans with absolute addresses, mean entropy, and the \
                 reasoning per span, plus a per-kind byte composition. These are HEURISTICS \
                 (confidence guess/likely) — corroborate with render_tiles (eyeball 'graphics'), \
                 the PC heatmap (confirm 'code'), and vram_to_rom (content match). `source` is a \
                 region NAME, rom/vram/memory (default rom), or \"rom_file\" (the on-disk CART — \
                 scans content the core hides, e.g. NES PRG+CHR). Read-only (no enable_writes).",
                scan_regions_schema(),
            ),
            Tool::new(
                "rom_info",
                "Parse the loaded ROM FILE's iNES / NES 2.0 header and report the cart layout: \
                 mapper, PRG/CHR sizes, and the exact FILE OFFSETS of PRG-ROM and CHR-ROM. Use \
                 this to point render_tiles/scan_regions at `rom_file:chr` (the graphics the \
                 core hides) without hand-computing offsets. CHR-RAM carts are flagged honestly \
                 (no CHR-ROM in the file). Non-NES ROMs return a note (raw rom_file still works). \
                 Read-only.",
                no_params(),
            ),
            Tool::new(
                "list_watches",
                "List the user-created memory watches.",
                no_params(),
            ),
            Tool::new("pause", "Pause emulation (safe control flag).", no_params()),
            Tool::new("resume", "Resume emulation (safe control flag).", no_params()),
            Tool::new(
                "step",
                "Advance emulation by exactly one frame while paused — SYNCHRONOUS: returns \
                 only once that frame has FULLY finished (all of run_frame's post-processing \
                 done), not merely queued or a counter that moved. An immediately-following \
                 hold_buttons/read is safe once this returns. Bounded wait (2s); a timeout \
                 means the emulation thread may be wedged. Backward compatible with callers \
                 that still poll get_state afterwards — they just observe it already landed.",
                no_params(),
            ),
            Tool::new(
                "run_frames",
                "Advance emulation by `count` frames SYNCHRONOUSLY in ONE call — the batch \
                 counterpart to `step`, for replay segments where N round trips would dominate \
                 the cost. Requires the emulator to already be PAUSED (same precondition as \
                 pause→step); refuses otherwise rather than silently pausing/resuming around \
                 you. `port0`/`port1`, if given, REPLACE that port's held set for the whole run \
                 (hold_buttons semantics) and stay held after the call returns; when given, this \
                 call blocks until the host loop confirms it has folded the NEW mask before \
                 counting frame 1, so the batch's first frame is guaranteed to run on the mask \
                 you just supplied, never a stale one from before the call (this closes a fixed \
                 race — do not work around it by adding your own settle/sleep). Returns \
                 {start_frame, end_frame, requested, landed, all_landed}. A load_state landing \
                 mid-run is not special-cased: frame_count is a host-side counter untouched by \
                 (un)serialization, so it just keeps counting through the load — only the \
                 CONTENT of frames after the load reflects the restored state. Capped at 600 \
                 frames per call (~10s at 60fps). Safe — input-only, no memory writes (no write \
                 gate). Residual hazard: like hold_buttons/press_buttons, input state is shared \
                 per session, not per-caller — a concurrent run_frames/hold_buttons from another \
                 MCP session between the mask being folded and this batch being armed can \
                 overwrite the mask before it's used. Single-agent-per-session by convention.",
                run_frames_schema(),
            ),
            Tool::new(
                "press_buttons",
                "Drive the game: HOLD controller buttons for `frames` emulated frames, so \
                 you can advance menus, START A MATCH, or perform moves in headless mode (no \
                 keyboard). Buttons in one call are held SIMULTANEOUSLY (e.g. [\"down\",\"b\"] for a \
                 special); call repeatedly to chain inputs. Names: a, b, x, y, l, r, start, select, \
                 up, down, left, right. `port` selects the controller: 0 = P1 (default), 1 = P2 (the \
                 second fighter / dummy slot). Emulation must be running (resume) for frames to \
                 advance. Safe — only feeds the controller, cannot corrupt memory (no write gate).",
                press_buttons_schema(),
            ),
            Tool::new(
                "hold_buttons",
                "Drive the game with a SUSTAINED hold: assert controller buttons on `port` on \
                 EVERY frame until release_buttons clears them, independent of press_buttons' \
                 frame countdown. Use this instead of press_buttons whenever game logic needs a \
                 CONTINUOUS hold to read correctly (e.g. guard checks) — the countdown decrements \
                 on every GUI frame including while paused, so it can drop out from under a \
                 pause→step sequence or a long single hold before it's actually consumed; a held \
                 button never decays. Idempotent: calling again REPLACES the port's held set \
                 (does not OR with the previous one) — pass [] to release everything on that \
                 port. `port`: 0 = P1 (default), 1 = P2. Safe — input-only, no memory writes \
                 (no write gate, same as press_buttons).",
                hold_buttons_schema(),
            ),
            Tool::new(
                "release_buttons",
                "Clear buttons asserted by hold_buttons. With `buttons`, releases just those \
                 names; omit `buttons` (or pass []) to release the ENTIRE held set for `port`. \
                 Does not touch an in-flight press_buttons countdown. Safe — no write gate.",
                release_buttons_schema(),
            ),
            Tool::new(
                "get_input",
                "Inspect `port`'s input pipeline: `asserted_*` is what the NEXT frame fold will \
                 feed the core (hold_buttons' held set OR'd with any live press_buttons \
                 countdown — non-consuming peek), `folded_*` is what the game ACTUALLY received \
                 on the LAST fold (also includes keyboard/pad in windowed mode). The two can \
                 legitimately differ while paused (asserted keeps showing the hold; folded is \
                 stale until the next step). Read-only, no write gate.",
                get_input_schema(),
            ),
            // ── input-slot record/playback (task A2) ────────────────────────
            Tool::new(
                "record_inputs",
                "Capture BOTH controller ports' folded per-frame input into a named slot \
                 (shadow/inputs/<family>/<name>.slot.json) — the frame lab's determinism \
                 instrument and a reproducible bug-repro format. `action=\"start\"` needs \
                 `name`; `action=\"stop\"` saves and returns {path, frames}. Captures exactly \
                 what the game received each REAL emulated frame (post keyboard/pad/MCP-\
                 injected/dummy fold), never on paused GUI frames. REQUIRES enable_writes.",
                record_inputs_schema(),
            ),
            Tool::new(
                "play_inputs",
                "Replay a slot saved by record_inputs deterministically onto one or both \
                 ports. `action=\"start\"` needs `name`; `port` (p1/p2/both, default both) \
                 chooses what it drives; `trigger` chooses when it begins (manual = next real \
                 frame — pair with pause/step for frame-exact timing; round_start = the fight \
                 gate's next closed→open transition, deterministic from a pre-round save \
                 state). If a training-mode dummy is also driving the targeted port, PLAYBACK \
                 WINS — the dummy's write is suppressed for that port, never blended. \
                 `action=\"stop\"` cancels an active/armed playback and releases its ports. \
                 REQUIRES enable_writes.",
                play_inputs_schema(),
            ),
            Tool::new(
                "list_input_slots",
                "List every input slot saved for the CURRENTLY LOADED family under \
                 shadow/inputs/<family>/ — name, frame count, created_at, and the best-effort \
                 save-state provenance recorded at capture time. Read-only, no enable_writes.",
                no_params(),
            ),
            // ── signal hunt ────────────────────────────────────────────────
            Tool::new(
                "hunt_configure",
                "Signal hunt (docs/signal-hunt.md §3): scope the RAM region that is snapshotted \
                 every frame, and set the analysis window. Default scope is the two fighter \
                 structs from the game profile; add `start`+`len` for a value that lives outside \
                 them (HUD mirrors, object arrays). REFUSES — by name and size — any region whose \
                 ring footprint would blow the budget, because a silently truncated hunt produces \
                 confident wrong answers. Changing the REGION discards marks captured under the \
                 old layout (their snapshots are not comparable); changing only ring/pre/post \
                 keeps them. Read-only, no write gate.",
                hunt_configure_schema(),
            ),
            Tool::new(
                "hunt_mark",
                "Signal hunt (§2): mark THIS frame with a label — \"event\" when the thing you are \
                 hunting just happened, \"control\" for a deliberate near-miss where it did NOT. \
                 Records frame, wall-clock, and the profile gate state, pins the snapshot from \
                 PRE frames ago, and schedules the POST capture. Marks are cheap; over-marking is \
                 fine. Judging 'that was a blocked hit' is YOUR job and cannot be automated — \
                 pretending otherwise is how false signals get shipped. Read-only, no write gate.",
                hunt_mark_schema(),
            ),
            Tool::new(
                "hunt_analyze",
                "Signal hunt (§4-§6): intersect the changed-sets of every event mark, subtract \
                 the union of the control marks' changed-sets plus idle churn, and rank what \
                 survives (fires-on-all, then small values, then counter-like, then \
                 byte-over-word). Returns per-mark value transitions for EVERY candidate so you \
                 can overrule the ranking, an evidence-doc markdown export, and the honesty \
                 fields: the settings used, gate-closed marks, unusable marks, and how many \
                 bytes each subtraction removed. ZERO CANDIDATES IS A RESULT. Addresses are \
                 profile-relative (block2+0x6F). This tool NEVER writes a profile — candidates \
                 are hypotheses until a write-test confirms them.",
                hunt_analyze_schema(),
            ),
            Tool::new(
                "hunt_reset",
                "Signal hunt: discard every mark and the snapshot ring (the region/window \
                 configuration survives). Read-only, no write gate.",
                no_params(),
            ),
            Tool::new(
                "run_lua",
                "Run a Lua script in the app's sandboxed engine on the main thread and return its console output. Gated/deferred round-trip.",
                run_lua_schema(),
            ),
            // ── save states ────────────────────────────────────────────────
            Tool::new(
                "save_state",
                "Snapshot the ENTIRE machine state (retro_serialize) to a file: slot 1-9 \
                 (<save_dir>/<rom>.state<N>, default slot 1) or an explicit `path`. Returns \
                 {ok, path, bytes}. Read-only w.r.t. the game (no enable_writes needed) — \
                 pair with load_state to bank and replay exact game situations.",
                state_target_schema(),
            ),
            Tool::new(
                "load_state",
                "Restore the machine from a save-state file (retro_unserialize): slot 1-9 \
                 or an explicit `path`. REPLACES the entire game state, so it REQUIRES \
                 enable_writes first. Bus-window snapshots are refreshed immediately after \
                 the load, so memory reads see the restored RAM on the same frame. Returns \
                 {ok, path, bytes}.",
                state_target_schema(),
            ),
            Tool::new(
                "load_shadow",
                "(Re)load the in-app shadow bot from a model directory (e.g. \
                 shadow/models/goat-v3) at runtime — validates cases.npz + meta.json and \
                 swaps the model without relaunching. Arrives DISABLED unless a shadow was \
                 already enabled (a swap keeps fighting); enabling is Shift+F5 or the \
                 Training panel. REQUIRES enable_writes (an active shadow's behavior \
                 changes immediately). Returns {ok, message}.",
                {
                    let schema = json!({
                        "type": "object",
                        "properties": {
                            "path": { "type": "string", "description": "Model directory containing cases.npz + meta.json" }
                        },
                        "required": ["path"]
                    });
                    Arc::new(schema.as_object().unwrap().clone())
                },
            ),
            // ── write gate + gated write/action tools ──────────────────────
            Tool::new(
                "enable_writes",
                "ARM the write tools for this session. Write tools (write_memory, freeze, \
                 unfreeze, set_breakpoint, clear_breakpoint, run_to) are LOCKED by default and \
                 refuse to act until you call this. This is the explicit confirm-before-write \
                 step. Call disable_writes to re-lock. A bad write can crash the core.",
                no_params(),
            ),
            Tool::new(
                "disable_writes",
                "Re-LOCK the write tools for this session (the default state).",
                no_params(),
            ),
            Tool::new(
                "write_memory",
                "Poke up to 4 little-endian bytes into guest memory at `addr`. REQUIRES \
                 enable_writes first (refused otherwise). Goes through the bounds-checked \
                 write path; refuses (without writing) if the target is read-only or unbacked. \
                 A bad write can crash the core.",
                write_memory_schema(),
            ),
            Tool::new(
                "freeze",
                "Freeze a guest address to a value: adds/updates a watch with frozen=true so \
                 the run loop re-writes it every frame (identical to the UI freeze checkbox). \
                 With `value`, freezes to that value; otherwise captures the current value. \
                 REQUIRES enable_writes first.",
                freeze_schema(),
            ),
            Tool::new(
                "unfreeze",
                "Clear the freeze on the watch at `addr` (like un-checking the UI freeze box). \
                 REQUIRES enable_writes first.",
                addr_only_schema(),
            ),
            Tool::new(
                "set_breakpoint",
                "Add an M68K PC breakpoint (deduped; capped at 8 to match the UI). The run \
                 loop pauses when the PC reaches it. REQUIRES enable_writes first.",
                addr_only_schema(),
            ),
            Tool::new(
                "clear_breakpoint",
                "Remove an M68K PC breakpoint. REQUIRES enable_writes first.",
                addr_only_schema(),
            ),
            Tool::new(
                "list_breakpoints",
                "List the current M68K PC breakpoints. Read-only (no enable_writes needed).",
                no_params(),
            ),
            Tool::new(
                "run_to",
                "Arm a one-shot run-to-address: emulation runs until the M68K PC reaches \
                 `addr`, then pauses. REQUIRES enable_writes first (it changes execution).",
                addr_only_schema(),
            ),
            // ── ROM-map writeback (persist findings across sessions) ─────────
            Tool::new(
                "get_rom_map",
                "Read-only: return the current literate ROM-map Markdown (frontmatter + \
                 ## Regions) for the loaded ROM so you can review what's already recorded. \
                 Reports exists=false (with the path) when no map has been scaffolded yet. \
                 No enable_writes needed.",
                no_params(),
            ),
            Tool::new(
                "add_rom_map_region",
                "Persist a CONFIRMED reverse-engineering finding into the ROM's literate \
                 Markdown map as an `author=ai` `::: region` block, so it survives across \
                 sessions instead of evaporating in chat. `kind` must be in the controlled \
                 vocabulary (rejected otherwise, with the valid list); `addr` is \"0xADDR\" or \
                 \"0xSTART-0xEND\"; `confidence` is confirmed|likely|guess (default likely). \
                 Scaffolds the map (frontmatter + ## Regions) if none exists, assigns a unique \
                 ai<n> id, and appends atomically — it NEVER rewrites existing human prose. \
                 REQUIRES enable_writes first (it mutates a file).",
                add_rom_map_region_schema(),
            ),
        ]
    }

    // ── resource catalog ───────────────────────────────────────────────────

    fn resources() -> Vec<RawResource> {
        let mk = |uri: &str, name: &str, desc: &str, mime: &str| {
            let mut r = RawResource::new(uri, name);
            r.description = Some(desc.to_string());
            r.mime_type = Some(mime.to_string());
            r
        };
        vec![
            mk("app://state", "App State", "JSON snapshot of the live app.", "application/json"),
            mk(
                "app://memory-map",
                "Memory Map",
                "JSON listing of every mapped region (name, kind ROM/RAM/VRAM/SRAM, \
                 addr_start/end, size, readonly). Read this first to orient before \
                 read_region / search_memory.",
                "application/json",
            ),
            mk("app://screen", "Screen", "Current framebuffer as a PNG image.", "image/png"),
            mk("app://watches", "Watches", "User memory watches as JSON.", "application/json"),
            mk("app://regions", "Code Regions", "User-labeled code regions as JSON.", "application/json"),
            mk("app://heatmap", "PC Heatmap", "Top hottest program counters as JSON.", "application/json"),
            mk("app://change-log", "Change Log", "Recent tracked-watch value changes as JSON.", "application/json"),
        ]
    }

    /// Resolve a resource URI to its contents. Shared by `read_resource`.
    fn read_resource_uri(&self, uri: &str) -> Result<Vec<ResourceContents>, ErrorData> {
        match uri {
            "app://state" => {
                let snap = {
                    let ds = self.lock_read()?;
                    AiSnapshot::from_debug_state(&ds)
                };
                let s = serde_json::to_string_pretty(&snap)
                    .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
                Ok(vec![ResourceContents::text(s, uri).with_mime_type("application/json")])
            }
            "app://memory-map" => {
                let payload = {
                    let ds = self.lock_read()?;
                    json!({
                        "capability": memory_capability(&ds),
                        "regions": memory_map(&ds),
                    })
                };
                let s = serde_json::to_string_pretty(&payload)
                    .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
                Ok(vec![ResourceContents::text(s, uri).with_mime_type("application/json")])
            }
            "app://screen" => {
                let (rgba, w, h) = {
                    let ds = self.lock_read()?;
                    (ds.fb_rgba.clone(), ds.fb_width, ds.fb_height)
                };
                let png = rgba_to_png(&rgba, w, h).ok_or_else(|| {
                    ErrorData::internal_error("no framebuffer available yet", None)
                })?;
                let b64 = base64_encode(&png);
                Ok(vec![ResourceContents::blob(b64, uri).with_mime_type("image/png")])
            }
            "app://watches" => {
                let watches = { self.lock_read()?.watches.clone() };
                let s = serde_json::to_string_pretty(&watches)
                    .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
                Ok(vec![ResourceContents::text(s, uri).with_mime_type("application/json")])
            }
            "app://regions" => {
                let regions = { self.lock_read()?.code_regions.clone() };
                let s = serde_json::to_string_pretty(&regions)
                    .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
                Ok(vec![ResourceContents::text(s, uri).with_mime_type("application/json")])
            }
            "app://heatmap" => {
                let top = {
                    let ds = self.lock_read()?;
                    top_heatmap(&ds, HEATMAP_TOP_N)
                };
                let s = serde_json::to_string_pretty(&top)
                    .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
                Ok(vec![ResourceContents::text(s, uri).with_mime_type("application/json")])
            }
            "app://change-log" => {
                let log: Vec<_> = { self.lock_read()?.change_log.iter().cloned().collect() };
                let s = serde_json::to_string_pretty(&log)
                    .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
                Ok(vec![ResourceContents::text(s, uri).with_mime_type("application/json")])
            }
            other => Err(ErrorData::resource_not_found(
                format!("unknown resource: {other}"),
                None,
            )),
        }
    }

    fn lock_read(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, crate::debug::DebugState>, ErrorData> {
        self.debug
            .lock()
            .map_err(|_| ErrorData::internal_error("debug state lock poisoned", None))
    }
}

impl ServerHandler for RetroMcpServer {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::default();
        info.protocol_version = ProtocolVersion::default();
        info.capabilities = ServerCapabilities::builder()
            .enable_tools()
            .enable_resources()
            .build();
        info.server_info = Implementation::new("rustretro-mcp", env!("CARGO_PKG_VERSION"));
        info.instructions = Some(
            "RustRetro live emulator/debugger. Use the `app://screen` resource to SEE the \
             game, `app://state` (or get_state) for registers/regions/counts, and \
             `app://memory-map` to orient on the regions. read_memory / read_region inspect \
             guest RAM/ROM/VRAM (read_region addresses by region NAME). search_memory finds a \
             byte pattern across regions, and vram_to_rom lifts VRAM bytes and content-searches \
             ROM for them — these are CONTENT matches, NOT DMA-traced provenance (the cores \
             expose no DMA hook). render_tiles is the SECOND, image-recognition evidence stream: \
             it decodes a ROM/VRAM span AS tiles (NES 2bpp / Genesis 4bpp) and returns a PNG \
             IMAGE so you can SEE the graphics and visually compare a candidate ROM region to \
             the sprite on app://screen — it survives compressed / re-bitplaned graphics where \
             vram_to_rom's verbatim byte match fails, so use BOTH for convergent evidence. \
             scan_regions is the THIRD, STRUCTURE stream: it windows a region and proposes a \
             KIND per span (packed/code/graphics/table/text/padding) from entropy + histogram \
             signatures, so you can ORIENT in an unknown ROM before zeroing in — corroborate its \
             guesses with render_tiles / the heatmap / vram_to_rom. To \
             answer 'which ROM holds the on-screen sprites': enumerate \
             the on-screen sprites' tile refs by writing a game-specific probe with run_lua \
             (see examples/cps2_oam_probe.lua), read those tiles out of VRAM with read_region, \
             then vram_to_rom/search_memory to get ROM candidates AND render_tiles to eyeball \
             the candidate region against the screen. pause/resume/step control \
             execution. Sprite/OAM layout is game- and system-specific; there is no universal \
             decoder. WRITE GATE: the write/action tools (write_memory, freeze, unfreeze, \
             set_breakpoint, clear_breakpoint, run_to) are LOCKED by default; call enable_writes \
             to arm them for this session (and disable_writes to re-lock). A bad write can crash \
             the core, so writes require this explicit confirm step. Read-only perception and \
             pause/resume/step/list_breakpoints stay available without arming. PERSIST FINDINGS: \
             once a region is CONFIRMED, durably record it in the ROM's literate Markdown map \
             with add_rom_map_region (gated — it writes a file, so enable_writes first); it \
             scaffolds the map if needed and appends an author=ai ::: region block without \
             touching existing prose. Review the current map any time with get_rom_map \
             (read-only). This is how findings survive across sessions instead of evaporating \
             in chat."
                .to_string(),
        );
        info
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListToolsResult, ErrorData>> + Send + '_ {
        async move {
            Ok(ListToolsResult {
                tools: Self::tools(),
                ..Default::default()
            })
        }
    }

    fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<CallToolResult, ErrorData>> + Send + '_ {
        let this = self.clone();
        async move {
            let name = request.name.as_ref();
            let args = request.arguments.unwrap_or_default();

            let get_u = |key: &str| -> Option<u64> { args.get(key).and_then(|v| v.as_u64()) };

            match name {
                "get_state" => {
                    let snap = {
                        let ds = this.lock_read()?;
                        AiSnapshot::from_debug_state(&ds)
                    };
                    Ok(CallToolResult::success(vec![Self::json_content(&snap)?]))
                }
                "read_memory" => {
                    let addr = get_u("addr").ok_or_else(|| {
                        ErrorData::invalid_params("missing/invalid `addr`", None)
                    })? as usize;
                    let len = get_u("len").ok_or_else(|| {
                        ErrorData::invalid_params("missing/invalid `len`", None)
                    })? as usize;
                    let v = this.read_memory(addr, len);
                    Ok(CallToolResult::success(vec![Self::json_content(&v)?]))
                }
                "list_regions" => {
                    let snap = {
                        let ds = this.lock_read()?;
                        AiSnapshot::from_debug_state(&ds)
                    };
                    Ok(CallToolResult::success(vec![Self::json_content(&snap.regions)?]))
                }
                "map_bus_window" => {
                    // addr/len accept a JSON integer or a hex string ("0x600000").
                    let num_arg = |key: &str| -> Option<u64> {
                        match args.get(key)? {
                            Value::Number(n) => n.as_u64(),
                            Value::String(s) => {
                                let t = s.trim();
                                let h = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X"))
                                    .unwrap_or(t);
                                u64::from_str_radix(h, 16).ok()
                            }
                            _ => None,
                        }
                    };
                    let name = args
                        .get("name")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| ErrorData::invalid_params("missing/invalid `name`", None))?
                        .to_string();
                    let addr = num_arg("addr").ok_or_else(|| {
                        ErrorData::invalid_params("missing/invalid `addr` (int or hex string)", None)
                    })?;
                    let len = num_arg("len").ok_or_else(|| {
                        ErrorData::invalid_params("missing/invalid `len` (int or hex string)", None)
                    })?;
                    const MAX_BUS_WINDOW: u64 = 1 << 20; // 1 MiB
                    if len == 0 || len > MAX_BUS_WINDOW {
                        return Err(ErrorData::invalid_params(
                            format!("`len` must be 1..={MAX_BUS_WINDOW} bytes"),
                            None,
                        ));
                    }
                    if addr > u32::MAX as u64 || addr + len > u32::MAX as u64 + 1 {
                        return Err(ErrorData::invalid_params(
                            "window must fit in the 32-bit bus",
                            None,
                        ));
                    }
                    let interval = get_u("interval").unwrap_or(1).max(1) as u32;
                    let cfg = crate::debug::BusWindowCfg {
                        name: name.clone(),
                        addr: addr as u32,
                        len: len as u32,
                        interval,
                        flags: crate::libretro::RETRO_MEMDESC_SYSTEM_RAM,
                    };
                    {
                        let mut ds = this.lock_read()?;
                        if ds.memory_regions.iter().any(|r| r.name == name)
                            || ds.pending_bus_windows.iter().any(|w| w.name == name)
                        {
                            return Err(ErrorData::invalid_params(
                                format!("a region named '{name}' already exists"),
                                None,
                            ));
                        }
                        ds.pending_bus_windows.push(cfg);
                        ds.save_busmap = true;
                    }
                    Ok(CallToolResult::success(vec![Self::json_content(&json!({
                        "ok": true,
                        "name": name,
                        "addr": format!("0x{addr:X}"),
                        "len": len,
                        "interval": interval,
                        "note": "window queued; the emulation thread installs and fills it \
                                 on the next frame (list_regions will then show it). If the \
                                 core lacks the bus-read API it stays zero-filled — check \
                                 the event log."
                    }))?]))
                }
                "read_region" => {
                    let region_name = args
                        .get("region_name")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| {
                            ErrorData::invalid_params("missing/invalid `region_name`", None)
                        })?
                        .to_string();
                    let offset = get_u("offset").unwrap_or(0) as usize;
                    let len = get_u("len").ok_or_else(|| {
                        ErrorData::invalid_params("missing/invalid `len`", None)
                    })? as usize;
                    let v = this.read_region(&region_name, offset, len);
                    Ok(CallToolResult::success(vec![Self::json_content(&v)?]))
                }
                "search_memory" => {
                    let needle_hex = args
                        .get("needle_hex")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| {
                            ErrorData::invalid_params("missing/invalid `needle_hex`", None)
                        })?;
                    let needle = parse_hex_bytes(needle_hex).ok_or_else(|| {
                        ErrorData::invalid_params(
                            "`needle_hex` must be an even-length hex string (separators allowed)",
                            None,
                        )
                    })?;
                    let scope = args.get("scope").and_then(|v| v.as_str()).unwrap_or("all");
                    let v = this.search_memory(&needle, scope);
                    Ok(CallToolResult::success(vec![Self::json_content(&v)?]))
                }
                "vram_to_rom" => {
                    let vram_addr = get_u("vram_addr").ok_or_else(|| {
                        ErrorData::invalid_params("missing/invalid `vram_addr`", None)
                    })? as usize;
                    let len = get_u("len").unwrap_or(32) as usize;
                    let v = this.vram_to_rom(vram_addr, len);
                    Ok(CallToolResult::success(vec![Self::json_content(&v)?]))
                }
                "render_tiles" => {
                    let source = args
                        .get("source")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| {
                            ErrorData::invalid_params("missing/invalid `source`", None)
                        })?;
                    let fmt_str =
                        args.get("format").and_then(|v| v.as_str()).ok_or_else(|| {
                            ErrorData::invalid_params("missing/invalid `format`", None)
                        })?;
                    let format = TileFormat::parse(fmt_str).ok_or_else(|| {
                        ErrorData::invalid_params(
                            format!(
                                "unknown tile `format` '{fmt_str}'; valid: {}",
                                TileFormat::valid_list()
                            ),
                            None,
                        )
                    })?;
                    let offset = get_u("offset").unwrap_or(0) as usize;
                    let len = get_u("len").unwrap_or(MAX_RENDER_TILES_LEN as u64) as usize;
                    let tiles_per_row =
                        get_u("tiles_per_row").unwrap_or(DEFAULT_TILES_PER_ROW as u64) as usize;
                    this.render_tiles(source, offset, len, format, tiles_per_row)
                }
                "scan_regions" => {
                    // `source` defaults to "rom" (the structure stream's usual target).
                    let source = args.get("source").and_then(|v| v.as_str()).unwrap_or("rom");
                    let window = get_u("window").unwrap_or(DEFAULT_SCAN_WINDOW as u64) as usize;
                    this.scan_regions(source, window)
                }
                "rom_info" => this.rom_info(),
                "list_watches" => {
                    let watches = { this.lock_read()?.watches.clone() };
                    Ok(CallToolResult::success(vec![Self::json_content(&watches)?]))
                }
                "pause" => Ok(CallToolResult::success(vec![Self::json_content(
                    &this.set_paused(true),
                )?])),
                "resume" => Ok(CallToolResult::success(vec![Self::json_content(
                    &this.set_paused(false),
                )?])),
                "step" => {
                    Ok(CallToolResult::success(vec![Self::json_content(&this.step())?]))
                }
                "run_frames" => {
                    let count = get_u("count").unwrap_or(0) as u32;
                    let port0_names: Option<Vec<String>> = args.get("port0").and_then(|v| v.as_array()).map(|a| {
                        a.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect()
                    });
                    let port1_names: Option<Vec<String>> = args.get("port1").and_then(|v| v.as_array()).map(|a| {
                        a.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect()
                    });
                    let port0_refs: Option<Vec<&str>> = port0_names.as_ref().map(|v| v.iter().map(|s| s.as_str()).collect());
                    let port1_refs: Option<Vec<&str>> = port1_names.as_ref().map(|v| v.iter().map(|s| s.as_str()).collect());
                    Ok(CallToolResult::success(vec![Self::json_content(
                        &this.run_frames(count, port0_refs.as_deref(), port1_refs.as_deref()),
                    )?]))
                }
                "press_buttons" => {
                    let buttons: Vec<String> = args
                        .get("buttons")
                        .and_then(|v| v.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                                .collect()
                        })
                        .unwrap_or_default();
                    if buttons.is_empty() {
                        return Err(ErrorData::invalid_params(
                            "missing/empty `buttons` array",
                            None,
                        ));
                    }
                    let frames = get_u("frames").unwrap_or(8) as u32;
                    let port = get_u("port").unwrap_or(0) as usize;
                    let refs: Vec<&str> = buttons.iter().map(|s| s.as_str()).collect();
                    Ok(CallToolResult::success(vec![Self::json_content(
                        &this.press_buttons(&refs, frames, port),
                    )?]))
                }
                "hold_buttons" => {
                    let buttons: Vec<String> = args
                        .get("buttons")
                        .and_then(|v| v.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                                .collect()
                        })
                        .unwrap_or_default();
                    let port = get_u("port").unwrap_or(0) as usize;
                    let refs: Vec<&str> = buttons.iter().map(|s| s.as_str()).collect();
                    Ok(CallToolResult::success(vec![Self::json_content(
                        &this.hold_buttons(&refs, port),
                    )?]))
                }
                "release_buttons" => {
                    let buttons: Vec<String> = args
                        .get("buttons")
                        .and_then(|v| v.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                                .collect()
                        })
                        .unwrap_or_default();
                    let port = get_u("port").unwrap_or(0) as usize;
                    let refs: Vec<&str> = buttons.iter().map(|s| s.as_str()).collect();
                    Ok(CallToolResult::success(vec![Self::json_content(
                        &this.release_buttons(&refs, port),
                    )?]))
                }
                "get_input" => {
                    let port = get_u("port").unwrap_or(0) as usize;
                    Ok(CallToolResult::success(vec![Self::json_content(
                        &this.get_input(port),
                    )?]))
                }
                // ── input-slot record/playback ───────────────────────────────
                "record_inputs" => {
                    let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("");
                    let name = args.get("name").and_then(|v| v.as_str());
                    Ok(CallToolResult::success(vec![Self::json_content(
                        &this.record_inputs(action, name),
                    )?]))
                }
                "play_inputs" => {
                    let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("");
                    let name = args.get("name").and_then(|v| v.as_str());
                    let port = args.get("port").and_then(|v| v.as_str()).unwrap_or("both");
                    let trigger = args.get("trigger").and_then(|v| v.as_str()).unwrap_or("manual");
                    Ok(CallToolResult::success(vec![Self::json_content(
                        &this.play_inputs(action, name, port, trigger),
                    )?]))
                }
                "list_input_slots" => Ok(CallToolResult::success(vec![Self::json_content(
                    &this.list_input_slots(),
                )?])),
                // ── signal hunt ─────────────────────────────────────────────
                "hunt_configure" => {
                    // start/len accept a JSON integer or a hex string, like
                    // map_bus_window — hunt regions are naturally hexadecimal.
                    let num_arg = |key: &str| -> Option<u64> {
                        match args.get(key)? {
                            Value::Number(n) => n.as_u64(),
                            Value::String(s) => {
                                let t = s.trim();
                                let h = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X"))
                                    .unwrap_or(t);
                                u64::from_str_radix(h, 16).ok()
                            }
                            _ => None,
                        }
                    };
                    let blocks = args.get("blocks").and_then(|v| v.as_bool()).unwrap_or(true);
                    let extra = match (num_arg("start"), num_arg("len")) {
                        (Some(a), Some(l)) => {
                            if a > u32::MAX as u64 || l > u32::MAX as u64 {
                                return Err(ErrorData::invalid_params(
                                    "`start`/`len` must fit in the 32-bit bus",
                                    None,
                                ));
                            }
                            Some((a as u32, l as u32))
                        }
                        (None, None) => None,
                        _ => {
                            return Err(ErrorData::invalid_params(
                                "`start` and `len` must be given together",
                                None,
                            ))
                        }
                    };
                    let v = this.hunt_configure(
                        blocks,
                        extra,
                        get_u("ring_frames").map(|n| n as usize),
                        get_u("pre"),
                        get_u("post"),
                        args.get("include_idle").and_then(|v| v.as_bool()),
                        args.get("enabled").and_then(|v| v.as_bool()),
                    );
                    Ok(CallToolResult::success(vec![Self::json_content(&v)?]))
                }
                "hunt_mark" => {
                    let label = args
                        .get("label")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| ErrorData::invalid_params("missing/invalid `label`", None))?;
                    Ok(CallToolResult::success(vec![Self::json_content(
                        &this.hunt_mark(label),
                    )?]))
                }
                "hunt_analyze" => {
                    let event = args
                        .get("event_label")
                        .and_then(|v| v.as_str())
                        .unwrap_or("event");
                    let control = args.get("control_label").and_then(|v| v.as_str());
                    Ok(CallToolResult::success(vec![Self::json_content(
                        &this.hunt_analyze(event, control),
                    )?]))
                }
                "hunt_reset" => {
                    Ok(CallToolResult::success(vec![Self::json_content(&this.hunt_reset())?]))
                }
                "run_lua" => {
                    let script = args
                        .get("script")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| {
                            ErrorData::invalid_params("missing/invalid `script`", None)
                        })?
                        .to_string();
                    let v = this.run_lua(script);
                    Ok(CallToolResult::success(vec![Self::json_content(&v)?]))
                }
                // ── save states ─────────────────────────────────────────────
                "save_state" | "load_state" => {
                    let slot = get_u("slot");
                    let path = args.get("path").and_then(|v| v.as_str());
                    let v = if name == "save_state" {
                        this.save_state(slot, path)
                    } else {
                        this.load_state(slot, path)
                    };
                    Ok(CallToolResult::success(vec![Self::json_content(&v)?]))
                }
                "load_shadow" => {
                    let path = args
                        .get("path")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| ErrorData::invalid_params("missing `path`", None))?;
                    let v = this.load_shadow(path);
                    Ok(CallToolResult::success(vec![Self::json_content(&v)?]))
                }
                // ── write gate ──────────────────────────────────────────────
                "enable_writes" => Ok(CallToolResult::success(vec![Self::json_content(
                    &this.enable_writes(),
                )?])),
                "disable_writes" => Ok(CallToolResult::success(vec![Self::json_content(
                    &this.disable_writes(),
                )?])),
                // ── gated write/action tools ────────────────────────────────
                "write_memory" => {
                    let addr = get_u("addr").ok_or_else(|| {
                        ErrorData::invalid_params("missing/invalid `addr`", None)
                    })? as usize;
                    let len = get_u("len").ok_or_else(|| {
                        ErrorData::invalid_params("missing/invalid `len`", None)
                    })? as usize;
                    let value = get_u("value").ok_or_else(|| {
                        ErrorData::invalid_params("missing/invalid `value`", None)
                    })? as u32;
                    let v = this.write_memory(addr, len, value);
                    Ok(CallToolResult::success(vec![Self::json_content(&v)?]))
                }
                "freeze" => {
                    let addr = get_u("addr").ok_or_else(|| {
                        ErrorData::invalid_params("missing/invalid `addr`", None)
                    })? as usize;
                    let fmt_str = args.get("format").and_then(|v| v.as_str()).ok_or_else(|| {
                        ErrorData::invalid_params("missing/invalid `format`", None)
                    })?;
                    let format = parse_watch_format(fmt_str).ok_or_else(|| {
                        ErrorData::invalid_params(
                            "`format` must be one of u8/s8/u16_le/u16_be/u32_le/u32_be/hex8/hex16/hex32",
                            None,
                        )
                    })?;
                    let value = get_u("value").map(|v| v as u32);
                    let v = this.freeze(addr, format, value);
                    Ok(CallToolResult::success(vec![Self::json_content(&v)?]))
                }
                "unfreeze" => {
                    let addr = get_u("addr").ok_or_else(|| {
                        ErrorData::invalid_params("missing/invalid `addr`", None)
                    })? as usize;
                    let v = this.unfreeze(addr);
                    Ok(CallToolResult::success(vec![Self::json_content(&v)?]))
                }
                "set_breakpoint" => {
                    let addr = get_u("addr").ok_or_else(|| {
                        ErrorData::invalid_params("missing/invalid `addr`", None)
                    })? as u32;
                    let v = this.set_breakpoint(addr);
                    Ok(CallToolResult::success(vec![Self::json_content(&v)?]))
                }
                "clear_breakpoint" => {
                    let addr = get_u("addr").ok_or_else(|| {
                        ErrorData::invalid_params("missing/invalid `addr`", None)
                    })? as u32;
                    let v = this.clear_breakpoint(addr);
                    Ok(CallToolResult::success(vec![Self::json_content(&v)?]))
                }
                "list_breakpoints" => Ok(CallToolResult::success(vec![Self::json_content(
                    &this.list_breakpoints(),
                )?])),
                "run_to" => {
                    let addr = get_u("addr").ok_or_else(|| {
                        ErrorData::invalid_params("missing/invalid `addr`", None)
                    })? as u32;
                    let v = this.run_to(addr);
                    Ok(CallToolResult::success(vec![Self::json_content(&v)?]))
                }
                // ── ROM-map writeback ───────────────────────────────────────
                "get_rom_map" => Ok(CallToolResult::success(vec![Self::json_content(
                    &this.get_rom_map(),
                )?])),
                "add_rom_map_region" => {
                    let kind = args.get("kind").and_then(|v| v.as_str()).ok_or_else(|| {
                        ErrorData::invalid_params("missing/invalid `kind`", None)
                    })?;
                    let addr = args.get("addr").and_then(|v| v.as_str()).ok_or_else(|| {
                        ErrorData::invalid_params("missing/invalid `addr`", None)
                    })?;
                    let label = args.get("label").and_then(|v| v.as_str());
                    let confidence = args.get("confidence").and_then(|v| v.as_str());
                    let note = args.get("note").and_then(|v| v.as_str());
                    let v = this.add_rom_map_region(kind, addr, label, confidence, note);
                    Ok(CallToolResult::success(vec![Self::json_content(&v)?]))
                }
                other => Err(ErrorData::invalid_params(
                    format!("unknown tool: {other}"),
                    None,
                )),
            }
        }
    }

    fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListResourcesResult, ErrorData>> + Send + '_ {
        async move {
            let resources = Self::resources()
                .into_iter()
                .map(|r| r.no_annotation())
                .collect();
            Ok(ListResourcesResult {
                resources,
                ..Default::default()
            })
        }
    }

    fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ReadResourceResult, ErrorData>> + Send + '_ {
        let this = self.clone();
        async move {
            let contents = this.read_resource_uri(&request.uri)?;
            Ok(ReadResourceResult::new(contents))
        }
    }
}

// Bring the `no_annotation()` extension into scope for RawResource → Resource.
use rmcp::model::AnnotateAble as _;

// ── standalone base64 (no extra dep) ─────────────────────────────────────────

/// Minimal standard-alphabet base64 encoder. Used to embed PNG bytes in the
/// `app://screen` blob resource. Kept local to avoid a new crate dependency.
fn base64_encode(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(T[((n >> 18) & 63) as usize] as char);
        out.push(T[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 { T[((n >> 6) & 63) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[(n & 63) as usize] as char } else { '=' });
    }
    out
}

/// Format a list of guest addresses as `0x`-prefixed hex strings for JSON output.
fn fmt_addrs(addrs: &[u32]) -> Vec<String> {
    addrs.iter().map(|a| format!("0x{a:X}")).collect()
}

/// Map a controller button NAME to its `RETRO_DEVICE_ID_JOYPAD` index (the index
/// into `DebugState.injected_input` / `Frontend.set_input`). Case-insensitive;
/// accepts a few friendly aliases. Returns `None` for an unknown name.
/// `pub(crate)`: the Lua `input.set` table form reuses the same name mapping.
pub(crate) fn joypad_button_index(name: &str) -> Option<usize> {
    match name.trim().to_ascii_lowercase().as_str() {
        "b" => Some(0),
        "y" => Some(1),
        "select" => Some(2),
        "start" => Some(3),
        "up" | "u" => Some(4),
        "down" | "d" => Some(5),
        "left" => Some(6),
        "right" => Some(7),
        "a" => Some(8),
        "x" => Some(9),
        "l" | "lb" | "l1" => Some(10),
        "r" | "rb" | "r1" => Some(11),
        _ => None,
    }
}

/// The friendly names for RETRO_DEVICE_ID_JOYPAD indices 0..=11, in index order
/// — the inverse of [`joypad_button_index`]. Used to decode a `[bool; 12]`
/// bitmap back into a human-readable button list for `get_input`.
const JOYPAD_NAMES: [&str; 12] =
    ["b", "y", "select", "start", "up", "down", "left", "right", "a", "x", "l", "r"];

/// Pack a `[bool; 12]` button bitmap into a `RETRO_DEVICE_ID_JOYPAD`-ordered
/// integer mask (bit i = button i), for hex display in `get_input`.
fn mask_from_bits(bits: &[bool; 12]) -> u32 {
    bits.iter().enumerate().fold(0u32, |m, (i, b)| m | ((*b as u32) << i))
}

/// Decode a `[bool; 12]` bitmap into the list of pressed buttons' names, in
/// `RETRO_DEVICE_ID_JOYPAD` order.
fn button_names(bits: &[bool; 12]) -> Vec<&'static str> {
    (0..12).filter(|&i| bits[i]).map(|i| JOYPAD_NAMES[i]).collect()
}

/// Map a case-insensitive format string to a [`WatchFormat`]. Accepts the names
/// of every `WatchFormat` variant plus a couple of friendly aliases. Used by the
/// `freeze` tool so an MCP-created watch matches the formats the UI offers.
fn parse_watch_format(s: &str) -> Option<WatchFormat> {
    match s.trim().to_ascii_lowercase().as_str() {
        "u8" => Some(WatchFormat::U8),
        "s8" | "i8" => Some(WatchFormat::S8),
        "u16" | "u16_le" | "u16le" => Some(WatchFormat::U16LE),
        "u16_be" | "u16be" => Some(WatchFormat::U16BE),
        "u32" | "u32_le" | "u32le" => Some(WatchFormat::U32LE),
        "u32_be" | "u32be" => Some(WatchFormat::U32BE),
        "hex8" | "hex_8" => Some(WatchFormat::Hex8),
        "hex16" | "hex_16" => Some(WatchFormat::Hex16),
        "hex32" | "hex_32" => Some(WatchFormat::Hex32),
        _ => None,
    }
}

/// Resolve the `save_state`/`load_state` arguments to a [`StateOp`]. Exactly one
/// of `slot` (1..=9, defaulting to 1 when both are absent) or `path` may be
/// given; slots are resolved to `<save_dir>/<rom_stem>.state<N>` by the
/// Frontend, which is the only side that knows save_dir. Pure and testable.
fn parse_state_target(slot: Option<u64>, path: Option<&str>, load: bool) -> Result<StateOp, String> {
    match (slot, path) {
        (Some(_), Some(_)) => Err("give `slot` OR `path`, not both".to_string()),
        (None, Some(p)) => {
            let p = p.trim();
            if p.is_empty() {
                return Err("`path` must be a non-empty file path".to_string());
            }
            let pb = std::path::PathBuf::from(p);
            Ok(if load { StateOp::Load(pb) } else { StateOp::Save(pb) })
        }
        (s, None) => {
            let n = s.unwrap_or(1);
            if !(1..=9).contains(&n) {
                return Err(format!("`slot` must be 1..=9 (got {n})"));
            }
            let n = n as u8;
            Ok(if load { StateOp::LoadSlot(n) } else { StateOp::SaveSlot(n) })
        }
    }
}

// ── ROM-map writeback helpers (pure, testable) ───────────────────────────────

/// Validate and normalize an `addr` token to its canonical form. Accepts a
/// single point `0xHHHH` or a range `0xSTART-0xEND`. Returns the normalized
/// uppercase-hex string (e.g. `"0x024000-0x025FFF"`) or an error message.
fn normalize_addr(addr: &str) -> Result<String, String> {
    let s = addr.trim();
    let parse_hex = |tok: &str| -> Result<u64, String> {
        let t = tok.trim();
        let body = t
            .strip_prefix("0x")
            .or_else(|| t.strip_prefix("0X"))
            .ok_or_else(|| format!("addr token '{t}' must be hex with a 0x prefix"))?;
        if body.is_empty() || !body.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(format!("addr token '{t}' is not valid hex"));
        }
        u64::from_str_radix(body, 16).map_err(|_| format!("addr token '{t}' out of range"))
    };

    if let Some((lo, hi)) = s.split_once('-') {
        let lo = parse_hex(lo)?;
        let hi = parse_hex(hi)?;
        if hi < lo {
            return Err(format!("addr range end 0x{hi:X} is before start 0x{lo:X}"));
        }
        Ok(format!("0x{lo:06X}-0x{hi:06X}"))
    } else {
        let p = parse_hex(s)?;
        Ok(format!("0x{p:06X}"))
    }
}

/// Build a minimal scaffold map (ROM_MAP_FORMAT §9) for a ROM that has no map
/// yet. Fills the identity fields we know (`rom.name`, `rom.sha1`) and leaves
/// unknowns blank/sensible. Always includes an (empty) `## Regions` section so
/// the first `append_region_block` has a home.
/// Build a fresh ROM-map markdown skeleton with frontmatter seeded from the
/// loaded ROM's identity. `rom_name`/`rom_sha1` come from `DebugState`; both may
/// be absent (e.g. need_fullpath cores never read the bytes), in which case we
/// emit empty strings — an empty value is an honest "human, please fill this"
/// signal, unlike the old misleading "unknown" placeholder.
///
/// `system` is intentionally left empty: only the running core knows the system,
/// and the ROM name/path doesn't reliably encode it, so guessing would be worse
/// than blank. `crc32` is likewise left for a human to fill.
///
/// The `rom:` block keys are nested under `rom:` with a 2-space indent so the
/// frontmatter parses as a valid YAML mapping (matching library/mvsc/mvsc.md).
fn scaffold_rom_map(
    rom_name: Option<&str>,
    rom_sha1: Option<&str>,
    rom_size: Option<usize>,
    rom_system: Option<&str>,
) -> String {
    let name = rom_name.unwrap_or("");
    let sha1 = rom_sha1.unwrap_or("");
    let size = rom_size.unwrap_or(0);
    // Inferred from the core's library_name; "" when unknown (multi-system cores)
    // — an honest blank a human can fill, matching the other empty-default fields.
    let system = rom_system.unwrap_or("");
    // NOTE: a raw string (not `"…\n\"` line-continuations) is required here —
    // the `\<newline>` continuation form strips the leading whitespace of the
    // following line, which silently flattened the indented YAML keys (the
    // "rom: keys not nested" bug). Raw strings preserve the 2-space indent.
    format!(
        r#"---
schema_version: 1

rom:
  name: "{name}"
  system: "{system}"
  sha1: "{sha1}"
  crc32: ""
  size: {size}

settings:
  scale: 3
  volume: 0.8
  muted: false
  breakpoints: []
  watches: []

meta:
  genre: ""
  year: ""
  developer: ""
  progress: "new"
  tags: []
---

# {name} — map

## Overview

_(notes go here)_

## Regions

_(region blocks accumulate here as you explore)_
"#
    )
}

/// Scan `existing` for `id=ai<N>` fence attributes and return the next free
/// `ai<N>` id (1-based, zero-padded to two digits like `ai01`). Avoids
/// collisions with any existing `ai`-prefixed id, including human-renamed ones.
fn next_ai_id(existing: &str) -> String {
    let mut max = 0u32;
    for line in existing.lines() {
        let line = line.trim_start();
        if !line.starts_with("::: region") {
            continue;
        }
        for tok in line.split_whitespace() {
            if let Some(val) = tok.strip_prefix("id=") {
                if let Some(num) = val.strip_prefix("ai") {
                    if let Ok(n) = num.parse::<u32>() {
                        max = max.max(n);
                    }
                }
            }
        }
    }
    format!("ai{:02}", max + 1)
}

/// PURE: given the current map Markdown, append a new `::: region` block to the
/// `## Regions` section and return the new content. Creates the `## Regions`
/// section (at the end) if missing. NEVER rewrites existing fence lines or
/// human prose (ROM_MAP_FORMAT §6) — it only appends. The opening fence carries
/// the AI authorship marker `author=<author>` so the block is reviewable.
#[allow(clippy::too_many_arguments)]
fn append_region_block(
    existing_md: &str,
    id: &str,
    kind: &str,
    addr: &str,
    label: Option<&str>,
    confidence: &str,
    author: &str,
    note: &str,
) -> String {
    // Build the fence line. Order: kind, id, addr, author, confidence, [label].
    let mut fence = format!(
        "::: region kind={kind} id={id} addr={addr} author={author} confidence={confidence}"
    );
    if let Some(lbl) = label {
        let lbl = lbl.trim();
        if !lbl.is_empty() {
            fence.push_str(&format!(" label=\"{}\"", lbl.replace('"', "'")));
        }
    }
    // The block: fence, one prose stub line (human-owned), closing fence.
    let block = format!("{fence}\n{note}\n:::\n");

    // Locate the `## Regions` heading (a line that is exactly `## Regions`,
    // ignoring trailing whitespace).
    let has_regions = existing_md
        .lines()
        .any(|l| l.trim_end() == "## Regions");

    if has_regions {
        // Append the block at the very end of the file, after all existing
        // content (which keeps every existing block + prose byte-for-byte).
        let mut out = String::with_capacity(existing_md.len() + block.len() + 2);
        out.push_str(existing_md);
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push('\n');
        out.push_str(&block);
        out
    } else {
        // No Regions section — create one at the end, then append the block.
        let mut out = String::with_capacity(existing_md.len() + block.len() + 32);
        out.push_str(existing_md);
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str("\n## Regions\n\n");
        out.push_str(&block);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::base64_encode;
    use super::{append_region_block, next_ai_id, normalize_addr, scaffold_rom_map};
    use super::{parse_state_target, parse_watch_format, RetroMcpServer};
    use crate::debug::{DebugState, WatchFormat};
    use serde_json::Value;
    use std::sync::{Arc, Mutex};

    #[test]
    fn base64_matches_known_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn rom_file_source_resolves_to_retained_bytes() {
        let mut ds = DebugState::new();
        ds.rom_name = Some("tmnt".into());
        ds.rom_bytes = Some(vec![1, 2, 3, 4, 5, 6, 7, 8]);
        let srv = RetroMcpServer::new(Arc::new(Mutex::new(ds)));

        // All accepted spellings resolve to the retained cart bytes.
        for tok in ["rom_file", "romfile", "ROM-FILE", " file "] {
            let (name, start, kind, bytes) =
                srv.resolve_source_bytes(tok).expect("rom_file should resolve");
            assert_eq!(start, 0, "tok {tok:?}");
            assert_eq!(kind, "ROMFILE", "tok {tok:?}");
            assert_eq!(bytes, vec![1, 2, 3, 4, 5, 6, 7, 8], "tok {tok:?}");
            assert!(name.contains("tmnt"), "name {name:?}");
        }

        // Token recognition is distinct from the region aliases.
        assert!(RetroMcpServer::is_rom_file_source("rom_file"));
        assert!(!RetroMcpServer::is_rom_file_source("rom"));
        assert!(!RetroMcpServer::is_rom_file_source("vram"));
        assert!(!RetroMcpServer::is_rom_file_source("PRG ROM"));
    }

    #[test]
    fn rom_file_source_errors_when_no_rom_available() {
        // No bytes, no path → honest error (not a panic, not empty bytes).
        let srv = RetroMcpServer::new(Arc::new(Mutex::new(DebugState::new())));
        let err = srv.resolve_source_bytes("rom_file").unwrap_err();
        assert!(err.contains("no ROM file"), "err {err:?}");
    }

    #[test]
    fn rom_file_part_splits_base_and_part() {
        use super::RetroMcpServer as S;
        assert_eq!(S::rom_file_part("rom_file"), Some(None));
        assert_eq!(S::rom_file_part(" ROM_FILE "), Some(None));
        assert_eq!(S::rom_file_part("rom_file:chr"), Some(Some("chr".into())));
        assert_eq!(S::rom_file_part("file:PRG"), Some(Some("prg".into())));
        assert_eq!(S::rom_file_part("rom"), None); // region alias, not the file
        assert_eq!(S::rom_file_part("PRG ROM"), None); // region name
    }

    #[test]
    fn rom_file_chr_span_slices_at_ines_offset() {
        // Build a tiny iNES cart: PRG=1×16KiB filled 0xAA, CHR=1×8KiB filled 0xCC.
        let mut rom = vec![0u8; 16];
        rom[0..4].copy_from_slice(&[0x4E, 0x45, 0x53, 0x1A]);
        rom[4] = 1; // PRG 16KiB
        rom[5] = 1; // CHR 8KiB
        rom[6] = 0x10; // mapper low nibble 1, no trainer
        rom.extend(std::iter::repeat(0xAA).take(0x4000)); // PRG
        rom.extend(std::iter::repeat(0xCC).take(0x2000)); // CHR
        let mut ds = DebugState::new();
        ds.rom_name = Some("toy".into());
        ds.rom_bytes = Some(rom);
        let srv = RetroMcpServer::new(Arc::new(Mutex::new(ds)));

        // rom_file:chr → the CHR span, addressed at its file offset (16 + 16KiB).
        let (name, start, kind, bytes) =
            srv.resolve_source_bytes("rom_file:chr").expect("chr span");
        assert_eq!(start, 16 + 0x4000);
        assert_eq!(kind, "ROMFILE:CHR");
        assert_eq!(bytes.len(), 0x2000);
        assert!(bytes.iter().all(|&b| b == 0xCC), "CHR should be all 0xCC");
        assert!(name.contains("chr"));

        // rom_file:prg → the PRG span (all 0xAA).
        let (_, pstart, pkind, pbytes) =
            srv.resolve_source_bytes("rom_file:prg").expect("prg span");
        assert_eq!(pstart, 16);
        assert_eq!(pkind, "ROMFILE:PRG");
        assert_eq!(pbytes.len(), 0x4000);
        assert!(pbytes.iter().all(|&b| b == 0xAA));
    }

    #[test]
    fn rom_file_chr_errors_on_chr_ram_cart() {
        // CHR size byte 0 → CHR-RAM; :chr must refuse with an honest message.
        let mut rom = vec![0u8; 16];
        rom[0..4].copy_from_slice(&[0x4E, 0x45, 0x53, 0x1A]);
        rom[4] = 1; // PRG
        rom[5] = 0; // CHR-RAM
        rom.extend(std::iter::repeat(0x00).take(0x4000));
        let mut ds = DebugState::new();
        ds.rom_bytes = Some(rom);
        let srv = RetroMcpServer::new(Arc::new(Mutex::new(ds)));
        let err = srv.resolve_source_bytes("rom_file:chr").unwrap_err();
        assert!(err.contains("CHR-RAM"), "err {err:?}");
    }

    #[test]
    fn joypad_button_index_maps_names_to_retro_ids() {
        use super::joypad_button_index as jb;
        // RETRO_DEVICE_ID_JOYPAD ordering.
        assert_eq!(jb("b"), Some(0));
        assert_eq!(jb("select"), Some(2));
        assert_eq!(jb("START"), Some(3)); // case-insensitive
        assert_eq!(jb("up"), Some(4));
        assert_eq!(jb("right"), Some(7));
        assert_eq!(jb("a"), Some(8));
        assert_eq!(jb("x"), Some(9));
        assert_eq!(jb(" l1 "), Some(10)); // alias + trim
        assert_eq!(jb("nope"), None);
    }

    #[test]
    fn press_buttons_sets_injected_frames_and_reports_unknown() {
        let srv = RetroMcpServer::new(Arc::new(Mutex::new(DebugState::new())));
        let v = srv.press_buttons(&["down", "b"], 8, 0);
        assert_eq!(v["ok"], true, "{v}");
        assert_eq!(v["port"], 0);
        {
            let ds = srv.debug.lock().unwrap();
            assert_eq!(ds.injected_input[5], 8); // down
            assert_eq!(ds.injected_input[0], 8); // b
            assert_eq!(ds.injected_input[3], 0); // start untouched
            assert_eq!(ds.injected_input2, [0u16; 12]); // P2 untouched
        }
        // Port 1 targets injected_input2, leaving P1 alone.
        let p2 = srv.press_buttons(&["right"], 8, 1);
        assert_eq!(p2["ok"], true, "{p2}");
        assert_eq!(p2["port"], 1);
        {
            let ds = srv.debug.lock().unwrap();
            assert_eq!(ds.injected_input2[7], 8); // right on P2
            assert_eq!(ds.injected_input[7], 0); // P1 right untouched
        }
        // Invalid port rejected.
        assert_eq!(srv.press_buttons(&["a"], 8, 2)["ok"], false);
        // Unknown button → error, nothing set.
        let e = srv.press_buttons(&["triangle"], 8, 0);
        assert_eq!(e["ok"], false);
        assert!(e["error"].as_str().unwrap().contains("unknown"));
        // Frames clamp to the max.
        let c = srv.press_buttons(&["a"], 99999, 0);
        assert_eq!(c["frames"], super::MAX_INPUT_HOLD_FRAMES as u16 as u64);
    }

    #[test]
    fn take_injected_input_holds_then_releases() {
        let mut ds = DebugState::new();
        ds.injected_input[3] = 2; // start, 2 frames
        assert_eq!(ds.take_injected_input()[3], true);
        assert_eq!(ds.injected_input[3], 1);
        assert_eq!(ds.take_injected_input()[3], true);
        assert_eq!(ds.take_injected_input()[3], false); // released
    }

    #[test]
    fn hold_buttons_asserts_until_release_and_replaces_not_ors() {
        let srv = RetroMcpServer::new(Arc::new(Mutex::new(DebugState::new())));
        let v = srv.hold_buttons(&["right"], 0);
        assert_eq!(v["ok"], true, "{v}");
        {
            let mut ds = srv.debug.lock().unwrap();
            // Survives MANY takes with no decay (unlike press_buttons' countdown).
            for _ in 0..20 {
                assert!(ds.take_injected_input()[7]);
            }
            assert_eq!(ds.take_injected_input2(), [false; 12], "P2 untouched");
        }
        // Calling again REPLACES the held set (does not OR with `right`).
        let v = srv.hold_buttons(&["up"], 0);
        assert_eq!(v["ok"], true, "{v}");
        {
            let mut ds = srv.debug.lock().unwrap();
            let f = ds.take_injected_input();
            assert!(f[4] && !f[7], "hold_buttons replaces, does not OR");
        }
        // Bad port / unknown button rejected without side effects.
        assert_eq!(srv.hold_buttons(&["a"], 2)["ok"], false);
        let e = srv.hold_buttons(&["triangle"], 0);
        assert_eq!(e["ok"], false);
        assert!(e["error"].as_str().unwrap().contains("unknown"));
    }

    #[test]
    fn release_buttons_clears_named_or_whole_port() {
        let srv = RetroMcpServer::new(Arc::new(Mutex::new(DebugState::new())));
        srv.hold_buttons(&["right", "up"], 0);
        let v = srv.release_buttons(&["right"], 0);
        assert_eq!(v["ok"], true, "{v}");
        {
            let mut ds = srv.debug.lock().unwrap();
            let f = ds.take_injected_input();
            assert!(!f[7] && f[4], "only right released");
        }
        let v = srv.release_buttons(&[], 0);
        assert_eq!(v["ok"], true, "{v}");
        assert_eq!(v["released"], "all");
        {
            let mut ds = srv.debug.lock().unwrap();
            assert_eq!(ds.take_injected_input(), [false; 12]);
        }
        assert_eq!(srv.release_buttons(&["a"], 2)["ok"], false);
    }

    // ── step / run_frames synchrony (task F1) ───────────────────────────────

    #[test]
    fn wait_for_next_frame_times_out_when_nothing_bumps_generation() {
        // No emulation thread exists in a unit test, so a wait that isn't
        // satisfied by an external notify MUST return Ok(None) — never hang —
        // within (well under) the requested bound. This is the bounded-wait
        // error path `step` surfaces as its timeout error.
        let srv = RetroMcpServer::new(Arc::new(Mutex::new(DebugState::new())));
        let start_gen = srv.debug.lock().unwrap().step_generation;
        let began = std::time::Instant::now();
        let got = srv.wait_for_next_frame(start_gen, std::time::Duration::from_millis(20));
        assert_eq!(got, Ok(None), "no notifier exists — must time out, not hang");
        assert!(began.elapsed() < std::time::Duration::from_millis(500), "must not block far past its timeout");
    }

    #[test]
    fn wait_for_next_frame_wakes_immediately_on_notify() {
        // Sanity-check the OTHER side of the same bound: when something DOES
        // bump `step_generation` and notify, the wait resolves promptly
        // instead of only ever resolving via timeout.
        let srv = RetroMcpServer::new(Arc::new(Mutex::new(DebugState::new())));
        let start_gen = srv.debug.lock().unwrap().step_generation;
        let debug = srv.debug.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(10));
            let mut ds = debug.lock().unwrap();
            ds.frame_count += 1;
            ds.step_generation = ds.step_generation.wrapping_add(1);
            ds.frame_cv.notify_all();
        });
        let began = std::time::Instant::now();
        let got = srv.wait_for_next_frame(start_gen, std::time::Duration::from_secs(2));
        assert_eq!(got, Ok(Some(1)), "should observe the bumped frame_count");
        assert!(began.elapsed() < std::time::Duration::from_secs(1), "should wake on notify, not timeout");
    }

    // ── run_frames mask/fold ordering (task F4 — closes the two-lock-
    //    acquisition race between setting held masks and arming the batch) ──

    #[test]
    fn wait_for_fold_times_out_when_nothing_bumps_generation() {
        // Mirrors wait_for_next_frame's timeout test but for fold_generation:
        // no host loop exists in a unit test, so an unsatisfied wait must
        // return Ok(None), never hang.
        let srv = RetroMcpServer::new(Arc::new(Mutex::new(DebugState::new())));
        let start_gen = srv.debug.lock().unwrap().fold_generation;
        let began = std::time::Instant::now();
        let got = srv.wait_for_fold(start_gen, std::time::Duration::from_millis(20));
        assert_eq!(got, Ok(None), "no fold notifier exists — must time out, not hang");
        assert!(began.elapsed() < std::time::Duration::from_millis(500), "must not block far past its timeout");
    }

    #[test]
    fn wait_for_fold_wakes_immediately_on_notify() {
        // The other side of the same bound: when something DOES bump
        // fold_generation and notify frame_cv, the wait resolves promptly.
        let srv = RetroMcpServer::new(Arc::new(Mutex::new(DebugState::new())));
        let start_gen = srv.debug.lock().unwrap().fold_generation;
        let debug = srv.debug.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(10));
            let mut ds = debug.lock().unwrap();
            ds.fold_generation = ds.fold_generation.wrapping_add(1);
            ds.frame_cv.notify_all();
        });
        let began = std::time::Instant::now();
        let got = srv.wait_for_fold(start_gen, std::time::Duration::from_secs(2));
        assert_eq!(got, Ok(Some(1)));
        assert!(began.elapsed() < std::time::Duration::from_secs(1), "should wake on notify, not timeout");
    }

    #[test]
    fn run_frames_with_masks_times_out_distinctly_when_the_fold_never_lands() {
        // If nothing ever folds the new masks (host loop wedged / test has no
        // stand-in thread at all), run_frames must report a fold-specific
        // timeout and MUST NOT have armed a batch — the whole point is that a
        // batch is never armed on an unconfirmed fold. Uses a temporarily
        // shortened fold timeout via direct field access is not exposed, so
        // this exercises the real FOLD_WAIT_TIMEOUT-bounded path; kept as a
        // single small case (not looped) to bound total suite time.
        let srv = RetroMcpServer::new(Arc::new(Mutex::new(DebugState::new())));
        srv.debug.lock().unwrap().paused = true;
        let v = srv.run_frames(1, Some(&["right"]), None);
        assert_eq!(v["ok"], false, "{v}");
        assert!(v["error"].as_str().unwrap().contains("fold"), "{v}");
        let ds = srv.debug.lock().unwrap();
        assert_eq!(ds.step_batch_remaining, 0, "must not arm a batch on an unconfirmed fold");
        assert!(ds.held_input[7], "the mask itself is still set — release_buttons can clear it");
    }

    #[test]
    fn run_frames_without_masks_does_not_wait_for_a_fold() {
        // The no-mask path is the one measured clean at 0/200 on the live
        // rig — it must stay a single lock acquisition with no fold wait, so
        // it keeps landing instantly with no dependency on the host loop
        // ever folding anything.
        let srv = RetroMcpServer::new(Arc::new(Mutex::new(DebugState::new())));
        srv.debug.lock().unwrap().paused = true;
        let debug = srv.debug.clone();
        std::thread::spawn(move || {
            loop {
                {
                    let ds = debug.lock().unwrap();
                    if ds.step_batch_remaining > 0 {
                        break;
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            let mut ds = debug.lock().unwrap();
            ds.step_batch_remaining = 0;
            ds.frame_count += 1;
            ds.step_generation = ds.step_generation.wrapping_add(1);
            ds.frame_cv.notify_all();
        });
        let began = std::time::Instant::now();
        let v = srv.run_frames(1, None, None);
        assert_eq!(v["ok"], true, "{v}");
        assert!(began.elapsed() < std::time::Duration::from_secs(1), "no fold wait means no multi-second latency");
    }

    #[test]
    fn run_frames_never_arms_the_batch_before_a_fold_confirms_the_new_masks() {
        // The direct regression test for the race: a watcher thread polls
        // for the specific bad interleaving (step_batch_remaining armed while
        // fold_generation is still at its pre-call value) that the OLD
        // single-lock-acquisition run_frames could produce. It must never be
        // observed with the fixed two-phase run_frames.
        let srv = RetroMcpServer::new(Arc::new(Mutex::new(DebugState::new())));
        srv.debug.lock().unwrap().paused = true;
        let debug = srv.debug.clone();

        let raced = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (raced_w, stop_w) = (raced.clone(), stop.clone());
        let debug_w = debug.clone();
        let watcher = std::thread::spawn(move || {
            while !stop_w.load(std::sync::atomic::Ordering::SeqCst) {
                let ds = debug_w.lock().unwrap();
                if ds.step_batch_remaining > 0 && ds.fold_generation == 0 {
                    raced_w.store(true, std::sync::atomic::Ordering::SeqCst);
                }
                drop(ds);
                std::thread::sleep(std::time::Duration::from_micros(50));
            }
        });

        std::thread::spawn(move || {
            // Stand in for the host loop: fold (bump fold_generation) only
            // after observing the new mask, then complete the batch.
            loop {
                {
                    let ds = debug.lock().unwrap();
                    if ds.held_input[7] {
                        break;
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            {
                let mut ds = debug.lock().unwrap();
                ds.fold_generation = ds.fold_generation.wrapping_add(1);
                ds.frame_cv.notify_all();
            }
            loop {
                {
                    let ds = debug.lock().unwrap();
                    if ds.step_batch_remaining > 0 {
                        break;
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            let mut ds = debug.lock().unwrap();
            ds.step_batch_remaining = 0;
            ds.frame_count += 1;
            ds.step_generation = ds.step_generation.wrapping_add(1);
            ds.frame_cv.notify_all();
        });

        let v = srv.run_frames(1, Some(&["right"]), None);
        stop.store(true, std::sync::atomic::Ordering::SeqCst);
        watcher.join().unwrap();

        assert_eq!(v["ok"], true, "{v}");
        assert!(
            !raced.load(std::sync::atomic::Ordering::SeqCst),
            "step_batch_remaining was armed while fold_generation was still at its pre-call \
             value — the batch's first frame could have run on a stale fold"
        );
    }

    #[test]
    fn step_is_synchronous_and_reports_the_landed_frame() {
        // Simulates the emulation thread: after `step` arms `step_one`, a
        // background thread stands in for `Frontend::run_frame` finishing the
        // frame and bumps+notifies `step_generation` exactly like the real
        // completion signal in frontend.rs. `step()` must not return before
        // that happens.
        let srv = RetroMcpServer::new(Arc::new(Mutex::new(DebugState::new())));
        let debug = srv.debug.clone();
        std::thread::spawn(move || {
            // Wait for step_one to be armed, mimicking the main loop noticing it.
            loop {
                {
                    let ds = debug.lock().unwrap();
                    if ds.step_one {
                        break;
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            let mut ds = debug.lock().unwrap();
            ds.step_one = false;
            ds.frame_count += 1;
            ds.step_generation = ds.step_generation.wrapping_add(1);
            ds.frame_cv.notify_all();
        });
        let v = srv.step();
        assert_eq!(v["ok"], true, "{v}");
        assert_eq!(v["landed"], true, "{v}");
        assert_eq!(v["frame_count"], 1, "{v}");
    }

    #[test]
    fn run_frames_rejects_zero_and_over_cap_count() {
        let srv = RetroMcpServer::new(Arc::new(Mutex::new(DebugState::new())));
        let v = srv.run_frames(0, None, None);
        assert_eq!(v["ok"], false, "{v}");
        assert!(v["error"].as_str().unwrap().contains(">= 1"), "{v}");

        let v = srv.run_frames(super::MAX_RUN_FRAMES + 1, None, None);
        assert_eq!(v["ok"], false, "{v}");
        assert!(v["error"].as_str().unwrap().contains("600"), "{v}");
    }

    #[test]
    fn run_frames_requires_paused() {
        // A fresh DebugState starts unpaused; run_frames must refuse rather
        // than silently pausing/resuming around the caller.
        let srv = RetroMcpServer::new(Arc::new(Mutex::new(DebugState::new())));
        assert!(!srv.debug.lock().unwrap().paused);
        let v = srv.run_frames(5, None, None);
        assert_eq!(v["ok"], false, "{v}");
        assert!(v["error"].as_str().unwrap().contains("paused"), "{v}");
        // And it must not have armed a batch it's about to refuse to run.
        assert_eq!(srv.debug.lock().unwrap().step_batch_remaining, 0);
    }

    #[test]
    fn run_frames_rejects_unknown_buttons_without_arming_a_batch() {
        let srv = RetroMcpServer::new(Arc::new(Mutex::new(DebugState::new())));
        srv.debug.lock().unwrap().paused = true;
        let v = srv.run_frames(5, Some(&["not_a_button"]), None);
        assert_eq!(v["ok"], false, "{v}");
        assert!(v["error"].as_str().unwrap().contains("unknown"), "{v}");
        assert_eq!(srv.debug.lock().unwrap().step_batch_remaining, 0, "must refuse before arming");
    }

    #[test]
    fn run_frames_times_out_and_reports_partial_progress() {
        // Nothing plays the role of the emulation thread here, so the batch
        // never completes — the bounded wait must still return (not hang)
        // and must clear step_batch_remaining rather than leave it dangling.
        let srv = RetroMcpServer::new(Arc::new(Mutex::new(DebugState::new())));
        let start_gen = {
            let mut ds = srv.debug.lock().unwrap();
            ds.paused = true;
            // Mirror what run_frames itself does right before it waits: arm
            // the batch so `step_batch_remaining == 0` doesn't look like an
            // (already-drained) batch that never started.
            ds.step_batch_remaining = 3;
            ds.step_generation
        };
        let (landed, end_frame, timed_out) = srv
            .wait_for_batch(start_gen, 3, std::time::Duration::from_millis(20))
            .unwrap();
        assert!(timed_out);
        assert_eq!(landed, 0);
        assert_eq!(end_frame, 0);
        assert_eq!(srv.debug.lock().unwrap().step_batch_remaining, 0, "timeout must clear the batch");
    }

    #[test]
    fn run_frames_applies_port_masks_and_reports_success_when_the_batch_lands() {
        // port0/port1 REPLACE the held set (same contract as hold_buttons),
        // applied before the batch is armed. A background thread stands in
        // for BOTH the host loop's fold (bumping fold_generation once the
        // new masks have landed — the ordering guarantee under test) and the
        // emulation thread completing the 1-frame batch, so this also
        // exercises run_frames' success (non-timeout) path end to end.
        let srv = RetroMcpServer::new(Arc::new(Mutex::new(DebugState::new())));
        srv.debug.lock().unwrap().paused = true;
        let debug = srv.debug.clone();
        std::thread::spawn(move || {
            // Stand in for the host loop's input fold (main.rs step (a0) /
            // read_input): wait for the masks run_frames just set, THEN bump
            // fold_generation — mirroring exactly what run_frames' internal
            // wait_for_fold blocks on before it will arm the batch.
            loop {
                {
                    let ds = debug.lock().unwrap();
                    if ds.held_input[7] && ds.held_input2[6] {
                        break;
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            {
                let mut ds = debug.lock().unwrap();
                ds.fold_generation = ds.fold_generation.wrapping_add(1);
                ds.frame_cv.notify_all();
            }
            // Stand in for the emulation thread completing the 1-frame batch.
            loop {
                {
                    let ds = debug.lock().unwrap();
                    if ds.step_batch_remaining > 0 {
                        break;
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            let mut ds = debug.lock().unwrap();
            ds.step_batch_remaining = 0;
            ds.frame_count += 1;
            ds.step_generation = ds.step_generation.wrapping_add(1);
            ds.frame_cv.notify_all();
        });
        let v = srv.run_frames(1, Some(&["right"]), Some(&["left"]));
        assert_eq!(v["ok"], true, "{v}");
        assert_eq!(v["landed"], 1, "{v}");
        assert_eq!(v["all_landed"], true, "{v}");
        let ds = srv.debug.lock().unwrap();
        assert!(ds.held_input[7], "port0 right should be held"); // right = index 7
        assert!(ds.held_input2[6], "port1 left should be held"); // left = index 6
    }

    #[test]
    fn get_input_reports_asserted_and_folded_separately() {
        let srv = RetroMcpServer::new(Arc::new(Mutex::new(DebugState::new())));
        srv.hold_buttons(&["right"], 0);
        srv.press_buttons(&["b"], 4, 0);
        {
            // `folded` mirrors what the run loop actually fed the core last —
            // simulate one fold the way main.rs/read_input do.
            let mut ds = srv.debug.lock().unwrap();
            ds.input_state = ds.take_injected_input();
        }
        let v = srv.get_input(0);
        assert_eq!(v["ok"], true, "{v}");
        assert_eq!(v["held_buttons"].as_array().unwrap(), &vec![Value::from("right")]);
        // asserted still includes `b`'s countdown (3 frames left after the fold above).
        let asserted: Vec<String> = v["asserted_buttons"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s.as_str().unwrap().to_string())
            .collect();
        assert!(asserted.contains(&"right".to_string()));
        assert!(asserted.contains(&"b".to_string()));
        // folded reflects the ONE fold already consumed above (both were live then).
        let folded: Vec<String> = v["folded_buttons"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s.as_str().unwrap().to_string())
            .collect();
        assert!(folded.contains(&"right".to_string()));
        assert!(folded.contains(&"b".to_string()));
        assert_eq!(srv.get_input(2)["ok"], false);
    }

    #[test]
    fn get_input_executed_is_sticky_across_non_executing_folds() {
        // task F4: `executed_*` (DebugState::last_executed_input) exists
        // because `folded_*` (input_state) is refreshed by `Frontend::
        // run_frame` on EVERY host-loop tick — including ticks whose frame
        // never actually ran — so a caller reading `folded` after a landed
        // `run_frames` batch can observe a value that has already drifted
        // away from what that specific executed frame saw. `executed` must
        // NOT drift: it changes only when something (standing in here for
        // `Frontend::run_frame`'s executed branch) explicitly sets it.
        let srv = RetroMcpServer::new(Arc::new(Mutex::new(DebugState::new())));
        {
            let mut ds = srv.debug.lock().unwrap();
            // Simulate frame 1 executing with "right" held: this is the
            // atomic capture `run_frame` does only on the branch that will
            // actually call core.run().
            ds.set_held_input(0, {
                let mut b = [false; 12];
                b[7] = true; // right
                b
            });
            ds.input_state = ds.take_injected_input();
            ds.last_executed_input = ds.input_state;
        }
        {
            // Simulate a LATER non-executing tick's re-fold with a DIFFERENT
            // mask — this is exactly the overwrite that made `folded` racy
            // as a post-hoc probe: it moves even though no frame ran.
            let mut ds = srv.debug.lock().unwrap();
            ds.set_held_input(0, {
                let mut b = [false; 12];
                b[6] = true; // left
                b
            });
            ds.input_state = ds.take_injected_input();
        }
        let v = srv.get_input(0);
        let folded: Vec<String> = v["folded_buttons"].as_array().unwrap()
            .iter().map(|s| s.as_str().unwrap().to_string()).collect();
        let executed: Vec<String> = v["executed_buttons"].as_array().unwrap()
            .iter().map(|s| s.as_str().unwrap().to_string()).collect();
        assert_eq!(folded, vec!["left".to_string()], "folded drifted to the later non-executing tick — expected");
        assert_eq!(executed, vec!["right".to_string()], "executed must stay pinned to what frame 1 actually saw");
    }

    #[test]
    fn parse_watch_format_maps_variants_and_aliases() {
        assert_eq!(parse_watch_format("u8"), Some(WatchFormat::U8));
        assert_eq!(parse_watch_format("S8"), Some(WatchFormat::S8));
        assert_eq!(parse_watch_format("i8"), Some(WatchFormat::S8));
        assert_eq!(parse_watch_format("u16"), Some(WatchFormat::U16LE));
        assert_eq!(parse_watch_format("u16_le"), Some(WatchFormat::U16LE));
        assert_eq!(parse_watch_format("U16BE"), Some(WatchFormat::U16BE));
        assert_eq!(parse_watch_format("u32_le"), Some(WatchFormat::U32LE));
        assert_eq!(parse_watch_format("u32be"), Some(WatchFormat::U32BE));
        assert_eq!(parse_watch_format(" hex16 "), Some(WatchFormat::Hex16));
        assert_eq!(parse_watch_format("hex32"), Some(WatchFormat::Hex32));
        assert_eq!(parse_watch_format("nope"), None);
    }

    #[test]
    fn write_gate_defaults_locked_and_arms() {
        let srv = RetroMcpServer::new(Arc::new(Mutex::new(DebugState::new())));
        // Default: locked.
        assert!(srv.check_writes_armed().is_err());
        // Arm.
        let _ = srv.enable_writes();
        assert!(srv.check_writes_armed().is_ok());
        // Re-lock.
        let _ = srv.disable_writes();
        assert!(srv.check_writes_armed().is_err());
    }

    #[test]
    fn gated_write_refused_when_locked_and_allowed_when_armed() {
        let srv = RetroMcpServer::new(Arc::new(Mutex::new(DebugState::new())));

        // Locked: a write/action tool must refuse WITHOUT touching state.
        let refused = srv.set_breakpoint(0x0400);
        assert_eq!(
            refused["error"].as_str(),
            Some("writes are locked; call enable_writes first")
        );
        assert!(srv.debug.lock().unwrap().breakpoints.is_empty());

        // run_to is likewise refused while locked.
        let refused_rt = srv.run_to(0x1000);
        assert!(refused_rt["error"].is_string());
        assert!(srv.debug.lock().unwrap().run_to_addr.is_none());

        // Arm, then the action succeeds and mutates state.
        let _ = srv.enable_writes();
        let ok = srv.set_breakpoint(0x0400);
        assert_eq!(ok["added"], serde_json::json!(true));
        assert!(srv.debug.lock().unwrap().breakpoints.contains(&0x0400));
    }

    // ── save states ─────────────────────────────────────────────────────────

    #[test]
    fn parse_state_target_resolves_slots_paths_and_errors() {
        use crate::debug::StateOp;
        use std::path::PathBuf;
        // Default: slot 1.
        assert_eq!(parse_state_target(None, None, false), Ok(StateOp::SaveSlot(1)));
        assert_eq!(parse_state_target(None, None, true), Ok(StateOp::LoadSlot(1)));
        // Explicit slot.
        assert_eq!(parse_state_target(Some(3), None, false), Ok(StateOp::SaveSlot(3)));
        assert_eq!(parse_state_target(Some(9), None, true), Ok(StateOp::LoadSlot(9)));
        // Explicit path.
        assert_eq!(
            parse_state_target(None, Some("/tmp/x.state"), false),
            Ok(StateOp::Save(PathBuf::from("/tmp/x.state")))
        );
        assert_eq!(
            parse_state_target(None, Some("/tmp/x.state"), true),
            Ok(StateOp::Load(PathBuf::from("/tmp/x.state")))
        );
        // Errors: both, out-of-range slot, empty path.
        assert!(parse_state_target(Some(1), Some("/tmp/x"), false).is_err());
        assert!(parse_state_target(Some(0), None, false).is_err());
        assert!(parse_state_target(Some(10), None, true).is_err());
        assert!(parse_state_target(None, Some("  "), false).is_err());
    }

    #[test]
    fn load_state_gated_save_state_validates_without_queueing() {
        let srv = RetroMcpServer::new(Arc::new(Mutex::new(DebugState::new())));
        // load_state is a write tool: refused while locked, nothing queued.
        let refused = srv.load_state(Some(1), None);
        assert_eq!(
            refused["error"].as_str(),
            Some("writes are locked; call enable_writes first")
        );
        assert!(srv.debug.lock().unwrap().pending_state_op.is_none());
        // save_state is not gated, but bad args error out before queueing.
        let bad = srv.save_state(Some(42), None);
        assert_eq!(bad["ok"], false);
        assert!(bad["error"].as_str().unwrap().contains("slot"));
        assert!(srv.debug.lock().unwrap().pending_state_op.is_none());
        // Armed load_state with bad args also errors without queueing.
        let _ = srv.enable_writes();
        let bad2 = srv.load_state(Some(1), Some("/x"));
        assert_eq!(bad2["ok"], false);
        assert!(srv.debug.lock().unwrap().pending_state_op.is_none());
    }

    // ── ROM-map writeback ───────────────────────────────────────────────────

    #[test]
    fn normalize_addr_accepts_point_and_range() {
        assert_eq!(normalize_addr("0x24000").unwrap(), "0x024000");
        assert_eq!(
            normalize_addr("0x024000-0x025FFF").unwrap(),
            "0x024000-0x025FFF"
        );
        // case-insensitive prefix + hex; whitespace trimmed.
        assert_eq!(normalize_addr(" 0XdeAD ").unwrap(), "0x00DEAD");
        // errors
        assert!(normalize_addr("24000").is_err()); // no 0x
        assert!(normalize_addr("0xZZ").is_err()); // not hex
        assert!(normalize_addr("0x200-0x100").is_err()); // end before start
    }

    #[test]
    fn append_preserves_existing_blocks_and_prose_and_marks_ai() {
        // A map WITH a ## Regions section containing a human block with prose.
        let existing = "\
---\nschema_version: 1\n---\n\n# Game — map\n\n## Regions\n\n\
::: region kind=title_screen id=tt01 addr=0x024000-0x025FFF confidence=confirmed\n\
Title tilemap, drawn by `title_draw`. DO NOT TOUCH THIS PROSE.\n\
:::\n";

        let out = append_region_block(
            existing,
            "ai01",
            "subroutine",
            "0x001000-0x0010FF",
            Some("hp_update"),
            "likely",
            "ai",
            "Found via heatmap + breakpoint.",
        );

        // Existing human block + its prose survive byte-for-byte.
        assert!(out.contains(
            "::: region kind=title_screen id=tt01 addr=0x024000-0x025FFF confidence=confirmed"
        ));
        assert!(out.contains("Title tilemap, drawn by `title_draw`. DO NOT TOUCH THIS PROSE."));
        // New block is present, tagged author=ai, with the note as prose.
        assert!(out.contains(
            "::: region kind=subroutine id=ai01 addr=0x001000-0x0010FF author=ai confidence=likely label=\"hp_update\""
        ));
        assert!(out.contains("Found via heatmap + breakpoint."));
        // The new block comes AFTER the existing one (no reordering).
        let tt = out.find("id=tt01").unwrap();
        let ai = out.find("id=ai01").unwrap();
        assert!(ai > tt);
    }

    #[test]
    fn append_creates_regions_section_when_missing() {
        let existing = "---\nschema_version: 1\n---\n\n# Game — map\n\n## Overview\n\nNotes.\n";
        let out = append_region_block(
            existing, "ai01", "palette", "0x008000", None, "guess", "ai", "A palette table.",
        );
        // Overview prose preserved.
        assert!(out.contains("## Overview"));
        assert!(out.contains("Notes."));
        // Regions section was created and holds the new block.
        assert!(out.contains("## Regions"));
        assert!(out.contains(
            "::: region kind=palette id=ai01 addr=0x008000 author=ai confidence=guess"
        ));
        // No label attr when label is None.
        assert!(!out.contains("label="));
    }

    #[test]
    fn next_ai_id_avoids_collision() {
        // Empty / no blocks → ai01.
        assert_eq!(next_ai_id("nothing here"), "ai01");
        // With an existing ai01, the next is ai02 (skips human tt01).
        let md = "\
## Regions\n\
::: region kind=subroutine id=ai01 addr=0x1000 author=ai confidence=likely\nx\n:::\n\
::: region kind=title_screen id=tt01 addr=0x2000 confidence=confirmed\ny\n:::\n";
        assert_eq!(next_ai_id(md), "ai02");
        // Highest wins even if non-contiguous.
        let md2 = "::: region kind=palette id=ai05 addr=0x3000 author=ai\nz\n:::\n";
        assert_eq!(next_ai_id(md2), "ai06");
    }

    #[test]
    fn scaffold_has_frontmatter_and_empty_regions() {
        let md = scaffold_rom_map(Some("mvsc"), Some("abc123"), Some(22699761), Some("cps2"));
        assert!(md.starts_with("---\nschema_version: 1"));
        // Identity fields are populated AND nested under `rom:` with a 2-space
        // indent so the frontmatter is a valid YAML mapping (not flat).
        assert!(md.contains("\nrom:\n"));
        assert!(md.contains("\n  name: \"mvsc\"\n"));
        assert!(md.contains("\n  sha1: \"abc123\"\n"));
        assert!(md.contains("\n  size: 22699761\n"));
        // `system` is populated when inferred (here "cps2"), never "unknown".
        assert!(md.contains("\n  system: \"cps2\"\n"));
        assert!(!md.contains("system: unknown"));
        // Verify the frontmatter block is well-formed: every key line between the
        // opening `---` and closing `---` that sits under `rom:` is 2-space
        // indented (a cheap stand-in for a YAML parse, since there's no yaml dep).
        let fm = md.split("---\n").nth(1).expect("frontmatter block");
        let mut in_rom = false;
        for line in fm.lines() {
            if line == "rom:" { in_rom = true; continue; }
            if line.is_empty() { continue; }
            // A new top-level key (no indent, ends the rom: block).
            if in_rom && !line.starts_with(' ') { in_rom = false; }
            if in_rom { assert!(line.starts_with("  "), "rom child not indented: {line:?}"); }
        }
        assert!(md.contains("## Regions"));

        // Missing identity falls back to empty strings / zero size, never "unknown".
        // A multi-system core (rom_system None) leaves `system` blank.
        let bare = scaffold_rom_map(None, None, None, None);
        assert!(bare.contains("\n  name: \"\"\n"));
        assert!(bare.contains("\n  sha1: \"\"\n"));
        assert!(bare.contains("\n  size: 0\n"));
        assert!(bare.contains("\n  system: \"\"\n"));
        assert!(!bare.contains("unknown"));
        // Round-trip: appending to a fresh scaffold yields a valid AI block.
        let out = append_region_block(
            &md, "ai01", "game_loop", "0x000400", None, "confirmed", "ai", "Main loop.",
        );
        assert!(out.contains(
            "::: region kind=game_loop id=ai01 addr=0x000400 author=ai confidence=confirmed"
        ));
    }

    // ── input-slot record/playback MCP tools (task A2) ──────────────────────

    #[test]
    fn record_inputs_requires_writes_armed() {
        crate::profile::init_for_tests();
        let srv = RetroMcpServer::new(Arc::new(Mutex::new(DebugState::new())));
        let v = srv.record_inputs("start", Some("locked-test"));
        assert_eq!(v["ok"], false);
        assert_eq!(v["error"], "writes are locked; call enable_writes first");
        assert!(srv.debug.lock().unwrap().recording_slot.is_none(), "must not queue while locked");
    }

    #[test]
    fn play_inputs_requires_writes_armed() {
        crate::profile::init_for_tests();
        let srv = RetroMcpServer::new(Arc::new(Mutex::new(DebugState::new())));
        let v = srv.play_inputs("start", Some("nope"), "both", "manual");
        assert_eq!(v["ok"], false);
        assert_eq!(v["error"], "writes are locked; call enable_writes first");
    }

    #[test]
    fn list_input_slots_is_ungated() {
        crate::profile::init_for_tests();
        let srv = RetroMcpServer::new(Arc::new(Mutex::new(DebugState::new())));
        // No enable_writes call at all — must still succeed (read-only).
        let v = srv.list_input_slots();
        assert_eq!(v["ok"], true);
        assert_eq!(v["family"], "asurabld");
    }

    #[test]
    fn record_inputs_start_stop_round_trips_a_slot_via_mcp() {
        crate::profile::init_for_tests();
        let srv = RetroMcpServer::new(Arc::new(Mutex::new(DebugState::new())));
        let _ = srv.enable_writes();

        let start = srv.record_inputs("start", Some("mcp-record-test"));
        assert_eq!(start["ok"], true, "{start}");

        // Advance two REAL frames the same way `training::tick` does
        // (`playback::tick`), exercising the MCP round-trip rather than
        // re-testing frame-driving mechanics already covered in
        // playback.rs's own unit tests.
        {
            let mut ds = srv.debug.lock().unwrap();
            ds.input_state[7] = true; // P1 Right
            crate::playback::tick(&mut ds, 1, crate::profile::current());
            ds.input_state = [false; 12];
            crate::playback::tick(&mut ds, 2, crate::profile::current());
        }

        let stop = srv.record_inputs("stop", None);
        assert_eq!(stop["ok"], true, "{stop}");
        assert_eq!(stop["frames"], 2);
        let path = stop["path"].as_str().unwrap().to_string();
        assert!(std::path::Path::new(&path).is_file());

        let list = srv.list_input_slots();
        assert_eq!(list["ok"], true);
        let names: Vec<String> = list["slots"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["name"].as_str().unwrap().to_string())
            .collect();
        assert!(names.contains(&"mcp-record-test".to_string()), "{names:?}");

        // A second start with the same MCP server now works again (proves
        // `stop` actually cleared the in-flight recording).
        let restart = srv.record_inputs("start", Some("mcp-record-test-2"));
        assert_eq!(restart["ok"], true);
        let stop2 = srv.record_inputs("stop", None);
        assert_eq!(stop2["frames"], 0, "no frames ticked — an empty slot is still valid");
        let path2 = stop2["path"].as_str().unwrap().to_string();

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&path2);
    }

    #[test]
    fn record_inputs_stop_without_start_errors() {
        crate::profile::init_for_tests();
        let srv = RetroMcpServer::new(Arc::new(Mutex::new(DebugState::new())));
        let _ = srv.enable_writes();
        let v = srv.record_inputs("stop", None);
        assert_eq!(v["ok"], false);
    }

    #[test]
    fn record_inputs_start_needs_a_name() {
        crate::profile::init_for_tests();
        let srv = RetroMcpServer::new(Arc::new(Mutex::new(DebugState::new())));
        let _ = srv.enable_writes();
        let v = srv.record_inputs("start", None);
        assert_eq!(v["ok"], false);
    }

    #[test]
    fn play_inputs_rejects_bad_port_and_trigger_before_touching_disk() {
        crate::profile::init_for_tests();
        let srv = RetroMcpServer::new(Arc::new(Mutex::new(DebugState::new())));
        let _ = srv.enable_writes();
        // The named slot need not even exist — validation runs before load.
        let v = srv.play_inputs("start", Some("whatever"), "p3", "manual");
        assert_eq!(v["ok"], false);
        assert!(v["error"].as_str().unwrap().contains("port"), "{v}");
        let v = srv.play_inputs("start", Some("whatever"), "both", "sometime");
        assert_eq!(v["ok"], false);
        assert!(v["error"].as_str().unwrap().contains("trigger"), "{v}");
    }

    #[test]
    fn play_inputs_missing_slot_errors_gracefully() {
        crate::profile::init_for_tests();
        let srv = RetroMcpServer::new(Arc::new(Mutex::new(DebugState::new())));
        let _ = srv.enable_writes();
        let v = srv.play_inputs("start", Some("this-slot-does-not-exist-anywhere"), "both", "manual");
        assert_eq!(v["ok"], false);
        assert!(srv.debug.lock().unwrap().playback_slot.is_none());
    }

    #[test]
    fn play_inputs_stop_without_one_active_errors() {
        crate::profile::init_for_tests();
        let srv = RetroMcpServer::new(Arc::new(Mutex::new(DebugState::new())));
        let _ = srv.enable_writes();
        let v = srv.play_inputs("stop", None, "both", "manual");
        assert_eq!(v["ok"], false);
    }

    #[test]
    fn record_then_play_round_trips_the_exact_sequence_via_mcp() {
        crate::profile::init_for_tests();
        let srv = RetroMcpServer::new(Arc::new(Mutex::new(DebugState::new())));
        let _ = srv.enable_writes();

        assert_eq!(srv.record_inputs("start", Some("mcp-e2e"))["ok"], true);
        {
            let mut ds = srv.debug.lock().unwrap();
            ds.input_state2[8] = true; // P2 A
            crate::playback::tick(&mut ds, 1, crate::profile::current());
            ds.input_state2 = [false; 12];
            crate::playback::tick(&mut ds, 2, crate::profile::current());
        }
        let stop = srv.record_inputs("stop", None);
        let path = stop["path"].as_str().unwrap().to_string();

        let play = srv.play_inputs("start", Some("mcp-e2e"), "p2", "manual");
        assert_eq!(play["ok"], true, "{play}");
        assert_eq!(play["frames"], 2);

        {
            let mut ds = srv.debug.lock().unwrap();
            crate::playback::tick(&mut ds, 3, crate::profile::current());
            assert!(ds.held_input2[8], "frame 1's P2 A came back out of the slot");
            crate::playback::tick(&mut ds, 4, crate::profile::current());
            assert!(!ds.held_input2[8], "frame 2 was idle");
            crate::playback::tick(&mut ds, 5, crate::profile::current());
            assert!(ds.playback_slot.is_none(), "one-shot playback finished and cleared itself");
        }

        let _ = std::fs::remove_file(&path);
    }
}
