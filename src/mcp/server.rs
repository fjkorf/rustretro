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

use crate::debug::SharedDebugState;
use crate::mcp::snapshot::{
    memory_map, parse_hex_bytes, rgba_to_png, search_bytes, top_heatmap, AiSnapshot,
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

/// The MCP server handler. Cloneable (it's just an `Arc` inside) so the
/// streamable-http service factory can mint a fresh handler per session.
#[derive(Clone)]
pub struct RetroMcpServer {
    debug: SharedDebugState,
}

impl RetroMcpServer {
    pub fn new(debug: SharedDebugState) -> Self {
        RetroMcpServer { debug }
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
    /// `(addr_start, kind, bytes)` or an error string. Reads byte-by-byte through
    /// the region's host pointer (the same path `read_addr` uses) so it works for
    /// any mapped region regardless of host-pointer arithmetic (offset/select).
    ///
    /// We deliberately materialize the bytes into an owned `Vec` so the caller can
    /// drop the mutex before doing any expensive scanning.
    fn clone_region_bytes(&self, region_name: &str) -> Result<(usize, String, Vec<u8>), String> {
        let ds = self.debug.lock().map_err(|_| "debug state lock poisoned".to_string())?;
        let region = ds
            .memory_regions
            .iter()
            .find(|r| r.name == region_name)
            .ok_or_else(|| format!("no region named '{region_name}'"))?
            .clone();
        let kind = region.region_type().to_string();
        let start = region.addr_start;
        let size = region.size.min(region.addr_end - region.addr_start + 1);
        let mut bytes = Vec::with_capacity(size);
        for off in 0..size {
            match ds.read_addr(start + off, 1) {
                Some(b) => bytes.push(b as u8),
                None => break, // hole / null host ptr — stop at what we have.
            }
        }
        drop(ds);
        Ok((start, kind, bytes))
    }

    /// `read_region`: read `len` bytes from within a NAMED region at `offset`.
    /// Lets Claude inspect VRAM/object-RAM/ROM by name without knowing absolute
    /// guest addresses. Caps `len` at [`MAX_REGION_READ_LEN`].
    fn read_region(&self, region_name: &str, offset: usize, len: usize) -> Value {
        let len = len.min(MAX_REGION_READ_LEN);
        let (start, kind, bytes) = match self.clone_region_bytes(region_name) {
            Ok(t) => t,
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
                let map = {
                    let ds = self.lock_read()?;
                    memory_map(&ds)
                };
                let s = serde_json::to_string_pretty(&map)
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
             expose no DMA hook). To answer 'which ROM holds the on-screen sprites': enumerate \
             the on-screen sprites' tile refs by writing a game-specific probe with run_lua \
             (see examples/cps2_oam_probe.lua), read those tiles out of VRAM with read_region, \
             then vram_to_rom/search_memory to get ROM candidates. pause/resume/step control \
             execution. Sprite/OAM layout is game- and system-specific; there is no universal \
             decoder."
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

#[cfg(test)]
mod tests {
    use super::base64_encode;

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
}
