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
use crate::mcp::snapshot::{rgba_to_png, top_heatmap, AiSnapshot};

/// How long the `run_lua` tool waits for the main thread to execute a script
/// before giving up and returning a timeout error.
const LUA_TIMEOUT: Duration = Duration::from_secs(5);
/// Cap on bytes returned by `read_memory` to avoid huge dumps.
const MAX_READ_LEN: usize = 4096;
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
             read_memory to inspect guest RAM/ROM. pause/resume/step control execution. \
             run_lua executes a sandboxed Lua script on the app's main thread."
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
