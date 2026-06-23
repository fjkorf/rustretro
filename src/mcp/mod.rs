//! AI Wave 1: an MCP (Model Context Protocol) server exposing the running
//! emulator/debugger to a Claude session.
//!
//! ## Transport
//! Streamable-HTTP over a localhost TCP port (default 4000). A GUI app can't use
//! stdio for MCP (Bevy owns the terminal), so we run an HTTP server on its own
//! thread + tokio runtime and let `claude mcp add` connect to it.
//!
//! ## Threading model (the important part)
//! `spawn_mcp_server` launches a `std::thread` with a fresh multi-thread tokio
//! runtime. The server holds a CLONE of `Arc<Mutex<DebugState>>` and only locks
//! it briefly. It never touches the NonSend `Emu`/`Lua` resources. The Bevy app
//! keeps running on the main thread, oblivious — when `--mcp` is absent this
//! module is never invoked and behavior is byte-for-byte identical.

pub mod ines;
pub mod server;
pub mod snapshot;

use std::sync::Arc;

use axum::Router;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService,
};

use crate::debug::SharedDebugState;
use server::RetroMcpServer;

/// Spawn the MCP server on its own OS thread with its own tokio runtime.
///
/// Returns immediately; the server runs for the lifetime of the process. A bind
/// failure (e.g. port in use) is logged to stderr and the thread exits — the
/// emulator continues unaffected.
pub fn spawn_mcp_server(debug: SharedDebugState, port: u16) {
    std::thread::Builder::new()
        .name("mcp-server".into())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    eprintln!("[mcp] failed to build tokio runtime: {e}");
                    return;
                }
            };
            rt.block_on(async move {
                if let Err(e) = serve(debug, port).await {
                    eprintln!("[mcp] server error: {e}");
                }
            });
        })
        .expect("failed to spawn mcp-server thread");
}

/// Build the axum router around the rmcp streamable-http service and serve it.
async fn serve(debug: SharedDebugState, port: u16) -> anyhow::Result<()> {
    // Per-session handler factory: each MCP session gets its own handler, all
    // sharing the same Arc<Mutex<DebugState>>.
    let factory = move || Ok(RetroMcpServer::new(debug.clone()));

    let service = StreamableHttpService::new(
        factory,
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default(),
    );

    let app = Router::new().nest_service("/mcp", service);

    let addr = format!("127.0.0.1:{port}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    eprintln!("[mcp] MCP server listening on http://{addr}/mcp");
    eprintln!("[mcp] connect with: claude mcp add --transport http rustretro http://{addr}/mcp");

    axum::serve(listener, app).await?;
    Ok(())
}
