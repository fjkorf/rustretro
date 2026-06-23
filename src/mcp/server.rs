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

use crate::debug::{SharedDebugState, Watch, WatchFormat};
use crate::mcp::snapshot::{
    decode_tiles_to_rgba, memory_capability, memory_map, parse_hex_bytes, read_region_bytes,
    rgba_to_png, scan_buffer, search_bytes, top_heatmap, AiSnapshot, TileFormat,
};

/// How long the `run_lua` tool waits for the main thread to execute a script
/// before giving up and returning a timeout error.
const LUA_TIMEOUT: Duration = Duration::from_secs(5);
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

    /// Arm the write tools for this session.
    fn enable_writes(&self) -> Value {
        self.writes_enabled.store(true, Ordering::SeqCst);
        json!({
            "ok": true,
            "writes_enabled": true,
            "message": "Write tools ARMED. write_memory/freeze/set_breakpoint/run_to are now \
                        active. Call disable_writes to re-lock.",
        })
    }

    /// Re-lock the write tools for this session.
    fn disable_writes(&self) -> Value {
        self.writes_enabled.store(false, Ordering::SeqCst);
        json!({
            "ok": true,
            "writes_enabled": false,
            "message": "Write tools LOCKED.",
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

        // Resolve `source` to a concrete region name via a brief lock.
        let region_name = self
            .resolve_render_source(source)
            .map_err(|e| ErrorData::invalid_params(e, None))?;

        // Clone the region bytes out (safe_host_ptr-guarded), then decode unlocked.
        let (start, kind, all_bytes) = match self.clone_region_bytes(&region_name) {
            Ok(t) => t,
            Err(e) if e.starts_with(REGION_UNBACKED) => {
                return Err(ErrorData::invalid_params(
                    format!(
                        "region '{region_name}' is declared but not backed by readable memory \
                         (virtual descriptor)"
                    ),
                    None,
                ))
            }
            Err(e) => return Err(ErrorData::invalid_params(e, None)),
        };

        if offset >= all_bytes.len() {
            return Err(ErrorData::invalid_params(
                format!(
                    "offset 0x{offset:X} is beyond region '{region_name}' (len 0x{:X})",
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

        let region_name = self
            .resolve_render_source(source)
            .map_err(|e| ErrorData::invalid_params(e, None))?;

        let (start, kind, all_bytes) = match self.clone_region_bytes(&region_name) {
            Ok(t) => t,
            Err(e) if e.starts_with(REGION_UNBACKED) => {
                return Err(ErrorData::invalid_params(
                    format!(
                        "region '{region_name}' is declared but not backed by readable memory \
                         (virtual descriptor)"
                    ),
                    None,
                ))
            }
            Err(e) => return Err(ErrorData::invalid_params(e, None)),
        };

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

    /// Request a single-frame step: clear pause-edge by arming `step_one`.
    fn step(&self) -> Value {
        if let Ok(mut ds) = self.debug.lock() {
            ds.step_one = true;
            json!({ "ok": true, "stepped": true })
        } else {
            json!({ "ok": false, "error": "lock poisoned" })
        }
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
                    "source": { "type": "string", "description": "Where to read bytes: an exact region NAME (see app://memory-map), or a convenience: \"rom\" / \"vram\" / \"memory\" (first ROM/VRAM/any backed region)" },
                    "offset": { "type": "integer", "description": "Byte offset WITHIN the source region (default 0)" },
                    "len":    { "type": "integer", "description": "Number of bytes to decode (capped at 65536)" },
                    "format": { "type": "string", "description": "Tile pixel format: 2bpp | nes_chr (NES, 16 B/tile, 4 colors) or 4bpp | genesis (Genesis, 32 B/tile, 16 colors)" },
                    "tiles_per_row": { "type": "integer", "description": "Tiles laid out per row in the image grid (default 16, max 64)" }
                },
                "required": ["source", "format"]
            });
            Arc::new(schema.as_object().unwrap().clone())
        };
        // Schema for { source, window? } (scan_regions).
        let scan_regions_schema = || -> Arc<Map<String, Value>> {
            let schema = json!({
                "type": "object",
                "properties": {
                    "source": { "type": "string", "description": "What to scan: an exact region NAME (see app://memory-map), or a convenience: \"rom\" / \"vram\" / \"memory\" (first ROM/VRAM/any backed region). Default \"rom\"." },
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
                 both for convergent evidence. `source` is a region NAME or rom/vram/memory; \
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
                 region NAME or rom/vram/memory (default rom). Read-only (no enable_writes).",
                scan_regions_schema(),
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
                "Advance emulation by one frame while paused (safe control flag).",
                no_params(),
            ),
            Tool::new(
                "run_lua",
                "Run a Lua script in the app's sandboxed engine on the main thread and return its console output. Gated/deferred round-trip.",
                run_lua_schema(),
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
    use super::{parse_watch_format, RetroMcpServer};
    use crate::debug::{DebugState, WatchFormat};
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
}
