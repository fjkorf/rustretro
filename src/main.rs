mod audio;
mod capstone_test;
mod phase2_test;
mod debug;
mod frontend;
mod libretro;
mod litui_pages;
mod lua_engine;
mod mcp;

use anyhow::Result;
use audio::AudioOutput;
use bevy::prelude::*;
use bevy::asset::RenderAssetUsages;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy_egui::{EguiContexts, EguiPlugin, EguiPrimaryContextPass};
use clap::Parser;
use debug::{DebugState, SharedDebugState};
use debug::panels::script_panel::ScriptPanel;
use frontend::Frontend;
use litui_pages::{LituiPages, TutorialPages};
use lua_engine::LuaEngine;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

// ─── CLI ─────────────────────────────────────────────────────────────────────

#[derive(Parser, Debug)]
#[command(name = "RustRetro", about = "Lightweight libretro frontend in Rust")]
struct Args {
    #[arg(long, value_name = "PATH")] core: String,
    #[arg(long, value_name = "PATH")] rom: String,
    #[arg(long)] fullscreen: bool,
    #[arg(long, value_name = "PATH", default_value = ".")] save_dir: PathBuf,
    #[arg(long, value_name = "PATH", default_value = ".")] system_dir: PathBuf,
    #[arg(long, value_name = "FACTOR", default_value = "3")] scale: u32,
    #[arg(long)] no_audio: bool,
    #[arg(long)] debug: bool,
    #[arg(long)] test_capstone: bool,
    #[arg(long)] test_phase2: bool,
    /// Optional Lua overlay script (loaded once at startup).
    #[arg(long, value_name = "PATH")] script: Option<PathBuf>,
    /// Expose the running app to a Claude session via an MCP server (AI Wave 1).
    #[arg(long)] mcp: bool,
    /// TCP port for the MCP server (only used with --mcp).
    #[arg(long, value_name = "N", default_value = "4000")] mcp_port: u16,
    /// Run with no window — emulator + MCP server only (for AI/agent-driven sessions). Implies --mcp.
    #[arg(long)] headless: bool,
}

// ─── Bevy resources ──────────────────────────────────────────────────────────

/// Emulation frontend — NonSend keeps retro_run() on the main thread.
struct Emu(Frontend);

/// Lua scripting engine — NonSend (mlua + Rc/RefCell are !Send), main-thread only.
struct LuaRes(LuaEngine);

#[derive(Resource)]
struct GameTexture(Handle<Image>);

#[derive(Resource)]
struct WindowScale(u32);

#[derive(Resource)]
struct DebugStateRes(SharedDebugState);

#[derive(Resource)]
struct DebugOverlay(debug::window::DebugApp);

#[derive(Resource)]
struct AudioRes(AudioOutput);

// ─── Entry point ─────────────────────────────────────────────────────────────

fn main() -> Result<()> {
    let mut args = Args::parse();

    // Headless without MCP is pointless (no window AND no agent channel). Auto-
    // enable MCP so `--headless` alone is sufficient for an agent-driven session.
    if args.headless && !args.mcp {
        args.mcp = true;
        eprintln!("[headless] --headless implies --mcp; enabling MCP server.");
    }

    // Run Capstone test if requested
    if args.test_capstone {
        capstone_test::run_capstone_tests();
        return Ok(());
    }

    // Run Phase 2 test if requested
    if args.test_phase2 {
        phase2_test::run_phase2_tests();
        return Ok(());
    }

    if !std::path::Path::new(&args.core).exists() { anyhow::bail!("Core not found: {}", args.core); }
    if !std::path::Path::new(&args.rom).exists()  { anyhow::bail!("ROM not found: {}", args.rom); }

    eprintln!("RustRetro — Bevy libretro frontend");
    eprintln!("Core: {}", args.core);
    eprintln!("ROM:  {}", args.rom);
    eprintln!("Press F12 to toggle debug overlay, Space to pause.");

    let debug_state: SharedDebugState = Arc::new(Mutex::new(DebugState::new()));

    let frontend = Frontend::new(
        &args.core, &args.rom,
        args.save_dir.clone(), args.system_dir.clone(),
        Arc::clone(&debug_state),
    )?;

    let w = frontend.video_width().max(320) * args.scale;
    let h = frontend.video_height().max(240) * args.scale;

    if args.debug { debug_state.lock().unwrap().debug_open = true; }

    // Build the Lua scripting engine (main-thread NonSend resource). Load the
    // optional --script once at startup. A failure to load logs but does not
    // abort the emulator.
    let mut lua_engine = LuaEngine::new(Arc::clone(&debug_state))
        .map_err(|e| anyhow::anyhow!("failed to init Lua engine: {e}"))?;
    if let Some(script_path) = &args.script {
        match lua_engine.load_script(&script_path.to_string_lossy()) {
            Ok(()) => eprintln!("Loaded Lua script: {}", script_path.display()),
            Err(e) => {
                eprintln!("Lua script load error ({}): {e}", script_path.display());
                debug_state.lock().unwrap().log(format!("[lua] load error: {e}"));
            }
        }
    }

    // AI Wave 1: optionally start the MCP server on its own thread. It holds a
    // clone of the Arc<Mutex<DebugState>> and locks it briefly to read; it never
    // touches the NonSend Emu/Lua resources. Absent --mcp, nothing changes.
    if args.mcp {
        mcp::spawn_mcp_server(Arc::clone(&debug_state), args.mcp_port);
    }

    // Headless mode: no window, no Bevy, no GPU. Run the emulator + Lua + MCP
    // round-trip on a plain main-thread loop. The MCP server was spawned above
    // (headless implies --mcp), so an agent connects exactly as in GUI mode —
    // there's just no window to crash or close. Return before building the App.
    if args.headless {
        return run_headless(frontend, lua_engine, debug_state, &args);
    }

    let fullscreen = if args.fullscreen {
        bevy::window::WindowMode::BorderlessFullscreen(MonitorSelection::Primary)
    } else {
        bevy::window::WindowMode::Windowed
    };

    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "RustRetro".to_string(),
                resolution: (w, h).into(),
                mode: fullscreen,
                ..default()
            }),
            ..default()
        }))
        .add_plugins(EguiPlugin::default())
        .insert_non_send_resource(Emu(frontend))
        .insert_resource(DebugStateRes(debug_state.clone()))
        .insert_resource(AudioRes(AudioOutput::new(!args.no_audio)))
        .insert_resource(WindowScale(args.scale))
        .insert_resource(DebugOverlay(debug::window::DebugApp::new(debug_state)))
        .insert_non_send_resource(LuaRes(lua_engine))
        .insert_resource(ScriptPanel::new())
        .init_resource::<LituiPages>()
        .init_resource::<TutorialPages>()
        .add_systems(Startup, setup)
        .add_systems(Update, (read_input, run_emulation, run_scripts, drain_lua_requests, sync_video, queue_audio, update_title).chain())
        .add_systems(EguiPrimaryContextPass, (show_debug, show_script_panel, show_litui_pages, show_tutorial_pages))
        .run();

    Ok(())
}

// ─── Headless mode (AI/agent-driven, no window) ──────────────────────────────

/// Run the emulator with NO GUI: a plain loop on the MAIN thread that ticks the
/// core + Lua frame callbacks and services the MCP `run_lua` round-trip, while
/// the MCP server (spawned in `main`) drives it from its own thread via the
/// shared `Arc<Mutex<DebugState>>`. Frontend and LuaEngine are !Send/main-thread
/// (libretro requires synchronous single-thread); they stay here and never move.
///
/// This replicates the non-GUI parts of the Bevy Update chain
/// (`run_emulation` → `run_scripts` → `drain_lua_requests`) so that MCP pause,
/// resume, step, memory reads, and `run_lua` all work identically — they read /
/// write `DebugState`, which this loop services every frame.
fn run_headless(
    mut frontend: Frontend,
    mut lua_engine: LuaEngine,
    debug_state: SharedDebugState,
    args: &Args,
) -> Result<()> {
    use std::time::{Duration, Instant};

    let fps = frontend.fps().max(1.0);
    let frame_dur = Duration::from_secs_f64(1.0 / fps);

    eprintln!(
        "[headless] running {fps:.0} fps, no window. MCP on http://127.0.0.1:{}/mcp. Ctrl-C to stop.",
        args.mcp_port
    );

    loop {
        let start = Instant::now();

        // (a) Tick the core one frame. run_frame() honours pause/step/trigger
        //     flags in DebugState internally, so MCP pause/resume/step work.
        let _ = frontend.run_frame();

        // (b) Lua per-frame callbacks, then composite their draw commands into
        //     `fb_rgba` — exactly like the GUI's run_scripts system. There IS a
        //     framebuffer in headless (run_frame() refreshed ds.fb_rgba above),
        //     and app://screen serves it, so an AGENT can SEE overlays (hitbox
        //     boxes, frame meter) it or a script draws. Draining without
        //     compositing previously made overlays GUI-only/invisible to MCP.
        let _ = lua_engine.run_frame_callbacks();
        let cmds = lua_engine.take_draw_cmds();
        if !cmds.is_empty() {
            if let Ok(mut ds) = debug_state.lock() {
                let (w, h) = (ds.fb_width, ds.fb_height);
                lua_engine::composite_into_rgba(&cmds, &mut ds.fb_rgba, w, h);
            }
        }

        // (c) Service the MCP run_lua round-trip — same logic as the GUI's
        //     drain_lua_requests system. WITHOUT this, MCP `run_lua` hangs (its
        //     5s poll times out). Runs even while paused, so an agent can pause
        //     then probe the running app.
        let pending = {
            match debug_state.lock() {
                Ok(mut ds) => ds.pending_lua.take(),
                Err(_) => None,
            }
        };
        if let Some(script) = pending {
            let result = lua_engine.eval_to_string(&script);
            if let Ok(mut ds) = debug_state.lock() {
                ds.pending_lua_result = Some(result);
            }
        }

        // (d) Frame pacing: sleep the remainder of the frame budget. When the
        //     core is paused, run_frame() returned early (cheap), so this still
        //     paces the loop at ~fps — the agent keeps a responsive run_lua /
        //     memory-read channel while paused.
        if let Some(rem) = frame_dur.checked_sub(start.elapsed()) {
            std::thread::sleep(rem);
        }
    }
}

// ─── Startup ─────────────────────────────────────────────────────────────────

fn setup(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    emu: NonSend<Emu>,
    scale: Res<WindowScale>,
) {
    commands.spawn(Camera2d::default());

    let gw = emu.0.video_width().max(320);
    let gh = emu.0.video_height().max(240);
    let s  = scale.0 as f32;

    let img = Image::new_fill(
        Extent3d { width: gw, height: gh, depth_or_array_layers: 1 },
        TextureDimension::D2,
        &[0, 0, 0, 255],
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    );
    let handle = images.add(img);
    commands.insert_resource(GameTexture(handle.clone()));
    commands.spawn(Sprite {
        image: handle,
        custom_size: Some(Vec2::new(gw as f32 * s, gh as f32 * s)),
        ..default()
    });
}

// ─── Input ───────────────────────────────────────────────────────────────────

fn read_input(
    keys: Res<ButtonInput<KeyCode>>,
    mut emu: NonSendMut<Emu>,
    debug_state: Res<DebugStateRes>,
    mut script_panel: ResMut<ScriptPanel>,
    mut litui: ResMut<LituiPages>,
    mut tutorials: ResMut<TutorialPages>,
) {
    use KeyCode::*;
    emu.0.set_input([
        keys.pressed(KeyZ),
        keys.pressed(KeyA),
        keys.pressed(ShiftLeft) || keys.pressed(ShiftRight),
        keys.pressed(Enter),
        keys.pressed(ArrowUp),
        keys.pressed(ArrowDown),
        keys.pressed(ArrowLeft),
        keys.pressed(ArrowRight),
        keys.pressed(KeyX),
        keys.pressed(KeyS),
        keys.pressed(KeyQ),
        keys.pressed(KeyW),
    ]);
    if keys.just_pressed(F12) {
        let mut ds = debug_state.0.lock().unwrap();
        ds.debug_open = !ds.debug_open;
    }
    if keys.just_pressed(Space) {
        let mut ds = debug_state.0.lock().unwrap();
        ds.paused = !ds.paused;
    }
    if keys.just_pressed(KeyB) {
        let mut ds = debug_state.0.lock().unwrap();
        ds.create_bookmark = true;
    }
    if keys.just_pressed(F10) {
        script_panel.open = !script_panel.open;
    }
    if keys.just_pressed(F9) {
        litui.open = !litui.open;
    }
    if keys.just_pressed(F8) {
        tutorials.open = !tutorials.open;
    }
}

// ─── Emulation ───────────────────────────────────────────────────────────────

fn run_emulation(mut emu: NonSendMut<Emu>) {
    let _ = emu.0.run_frame();
}

// ─── Scripting ───────────────────────────────────────────────────────────────

/// Run Lua per-frame callbacks and composite their draw commands into the fresh
/// framebuffer. Sits BETWEEN run_emulation (which refreshes fb_rgba) and
/// sync_video (which uploads it), so overlays never lag a frame.
/// Render the Lua script panel (floating window). Separate from the tab-based
/// DebugApp because LuaEngine is a !Send NonSend resource and can't thread
/// through the Send DebugApp.
fn show_script_panel(
    mut ctx: EguiContexts,
    mut lua: NonSendMut<LuaRes>,
    mut panel: ResMut<ScriptPanel>,
    debug_state: Res<DebugStateRes>,
) {
    if let Ok(ctx) = ctx.ctx_mut() {
        panel.show(ctx, &mut lua.0, &debug_state.0);
    }
}

/// AI Wave 1: pick up a Lua script submitted by the MCP `run_lua` tool, run it
/// on the main thread (where the NonSend LuaEngine lives), and write the result
/// back for the MCP thread to poll. A no-op when no request is pending, so it's
/// free when --mcp is absent. Errors are isolated to the result channel.
fn drain_lua_requests(lua: NonSend<LuaRes>, debug_state: Res<DebugStateRes>) {
    // Take the pending request under a brief lock.
    let script = {
        let Ok(mut ds) = debug_state.0.lock() else { return };
        ds.pending_lua.take()
    };
    let Some(script) = script else { return };

    let result = lua.0.eval_to_string(&script);

    if let Ok(mut ds) = debug_state.0.lock() {
        ds.pending_lua_result = Some(result);
    }
}

fn run_scripts(lua: NonSend<LuaRes>, debug_state: Res<DebugStateRes>) {
    let _ = lua.0.run_frame_callbacks();
    let cmds = lua.0.take_draw_cmds();
    if cmds.is_empty() {
        return;
    }
    if let Ok(mut ds) = debug_state.0.lock() {
        let (w, h) = (ds.fb_width, ds.fb_height);
        lua_engine::composite_into_rgba(&cmds, &mut ds.fb_rgba, w, h);
    }
}

// ─── Video ───────────────────────────────────────────────────────────────────

/// Convert any libretro pixel format → RGBA8 bytes (row-major, top-down).
fn to_rgba8(src: &[u8], w: u32, h: u32, pitch: usize, fmt: u32) -> Vec<u8> {
    let (w, h) = (w as usize, h as usize);
    let mut out = vec![0u8; w * h * 4];
    for y in 0..h {
        let ri = &src[y * pitch..];
        let ro = &mut out[y * w * 4..];
        match fmt {
            2 => for x in 0..w { // RGB565
                let p = (ri[x*2] as u16) | ((ri[x*2+1] as u16) << 8);
                ro[x*4]   = ((p >> 11) & 0x1F) as u8 * 8;
                ro[x*4+1] = ((p >>  5) & 0x3F) as u8 * 4;
                ro[x*4+2] = ( p        & 0x1F) as u8 * 8;
                ro[x*4+3] = 0xFF;
            },
            1 => for x in 0..w { // XRGB8888: memory [B,G,R,X]
                ro[x*4]   = ri[x*4+2]; // R
                ro[x*4+1] = ri[x*4+1]; // G
                ro[x*4+2] = ri[x*4];   // B
                ro[x*4+3] = 0xFF;
            },
            _ => for x in 0..w { // 0RGB1555
                let p = (ri[x*2] as u16) | ((ri[x*2+1] as u16) << 8);
                ro[x*4]   = ((p >> 10) & 0x1F) as u8 * 8;
                ro[x*4+1] = ((p >>  5) & 0x1F) as u8 * 8;
                ro[x*4+2] = ( p        & 0x1F) as u8 * 8;
                ro[x*4+3] = 0xFF;
            },
        }
    }
    out
}

fn sync_video(
    emu: NonSend<Emu>,
    game_tex: Res<GameTexture>,
    mut images: ResMut<Assets<Image>>,
    scale: Res<WindowScale>,
    debug_state: Res<DebugStateRes>,
    mut sprites: Query<&mut Sprite>,
) {
    let Some((fb, w, h, pitch, fmt)) = emu.0.framebuffer() else { return };
    // Prefer the DebugState's RGBA framebuffer when it's fresh and matches the
    // core dimensions: run_scripts has already composited Lua overlays onto it
    // this frame. Fall back to decoding the raw core framebuffer otherwise.
    let rgba = {
        let composited = debug_state.0.lock().ok().and_then(|ds| {
            if ds.fb_width == w && ds.fb_height == h && ds.fb_rgba.len() == (w * h * 4) as usize {
                Some(ds.fb_rgba.clone())
            } else {
                None
            }
        });
        composited.unwrap_or_else(|| to_rgba8(fb, w, h, pitch, fmt))
    };

    if let Some(img) = images.get_mut(&game_tex.0) {
        if img.width() != w || img.height() != h {
            let s = scale.0 as f32;
            *img = Image::new_fill(
                Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                TextureDimension::D2,
                &[0, 0, 0, 255],
                TextureFormat::Rgba8UnormSrgb,
                RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
            );
            if let Ok(mut sp) = sprites.single_mut() {
                sp.custom_size = Some(Vec2::new(w as f32 * s, h as f32 * s));
            }
        }
        if let Some(data) = img.data.as_mut() {
            if data.len() == rgba.len() {
                data.copy_from_slice(&rgba);
            }
        }
    }
}

// ─── Audio ───────────────────────────────────────────────────────────────────

fn queue_audio(mut emu: NonSendMut<Emu>, audio: Res<AudioRes>) {
    let samples = emu.0.drain_audio();
    audio.0.queue(&samples);
}

// ─── Debug overlay ───────────────────────────────────────────────────────────

fn show_debug(
    mut ctx: EguiContexts,
    debug_state: Res<DebugStateRes>,
    audio: Res<AudioRes>,
    mut overlay: ResMut<DebugOverlay>,
    mut audio_wired: Local<bool>,
) {
    // Wire the audio panel exactly once. `AudioOutput` now shares volume/mute via
    // `Arc<Atomic*>`, so this clone observes (and mutates) the same state the player
    // uses. Running it every frame would churn a fresh mutex each frame, so guard it.
    if !*audio_wired {
        overlay.0.set_audio(Arc::new(Mutex::new(audio.0.clone())));
        *audio_wired = true;
    }
    let open = debug_state.0.lock().map(|s| s.debug_open).unwrap_or(false);
    if open {
        if let Ok(ctx) = ctx.ctx_mut() {
            overlay.0.show(ctx);
        }
    }
}

// ─── litui preview pages (Wave C) ────────────────────────────────────────────

/// Render the three litui Markdown pages (CPU / Log / Audio) and run the
/// live-resource binding. This is the Wave C deliverable: a per-frame projection
/// of the shared `DebugState` into the macro-generated `AppState`, plus the Audio
/// form round-trip (widget outputs → live `AudioOutput`). Gated by F9; a no-op
/// when closed, so existing behaviour is unchanged.
fn show_litui_pages(
    mut ctx: EguiContexts,
    debug_state: Res<DebugStateRes>,
    mut audio: ResMut<AudioRes>,
    mut litui: ResMut<LituiPages>,
) {
    if !litui.open {
        return;
    }
    sync_litui_pages(&mut litui, &debug_state.0, &mut audio.0);
    if let Ok(ctx) = ctx.ctx_mut() {
        litui.md.show_all(ctx);
    }
}

/// Wave D: render the in-app tutorials (Help → Tutorials). These are static
/// litui document pages authored in `docs/tutorials/` — no live binding needed.
/// Gated by F8; a no-op when closed, so existing behaviour is unchanged.
fn show_tutorial_pages(mut ctx: EguiContexts, mut tutorials: ResMut<TutorialPages>) {
    if !tutorials.open {
        return;
    }
    if let Ok(ctx) = ctx.ctx_mut() {
        tutorials.md.show_all(ctx);
    }
}

/// The entire per-frame sync glue: DebugState values DOWN into the litui AppState,
/// and Audio form widget outputs UP into the live AudioOutput. Kept in one small
/// function (the "measure the sync glue" risk from the ROADMAP).
fn sync_litui_pages(litui: &mut LituiPages, debug_state: &SharedDebugState, audio: &mut AudioOutput) {
    let s = &mut litui.md.state;

    // ── widget outputs UP: Audio form → live AudioOutput ──
    audio.set_mute(s.mute);
    audio.set_volume(s.volume as f32);

    // ── values DOWN: DebugState / AudioOutput → [display] fields ──
    let Ok(ds) = debug_state.lock() else { return };

    s.d0 = format!("{:08X}", ds.m68k_d_regs[0]);
    s.d1 = format!("{:08X}", ds.m68k_d_regs[1]);
    s.d2 = format!("{:08X}", ds.m68k_d_regs[2]);
    s.d3 = format!("{:08X}", ds.m68k_d_regs[3]);
    s.d4 = format!("{:08X}", ds.m68k_d_regs[4]);
    s.d5 = format!("{:08X}", ds.m68k_d_regs[5]);
    s.d6 = format!("{:08X}", ds.m68k_d_regs[6]);
    s.d7 = format!("{:08X}", ds.m68k_d_regs[7]);
    s.a0 = format!("{:08X}", ds.m68k_a_regs[0]);
    s.a1 = format!("{:08X}", ds.m68k_a_regs[1]);
    s.a2 = format!("{:08X}", ds.m68k_a_regs[2]);
    s.a3 = format!("{:08X}", ds.m68k_a_regs[3]);
    s.a4 = format!("{:08X}", ds.m68k_a_regs[4]);
    s.a5 = format!("{:08X}", ds.m68k_a_regs[5]);
    s.a6 = format!("{:08X}", ds.m68k_a_regs[6]);
    s.a7 = format!("{:08X}", ds.m68k_a_regs[7]);
    s.pc = format!("{:08X}", ds.m68k_pc);
    s.sr = format!("{:04X}", ds.m68k_sr);
    s.z80_pc = format!("{:04X}", ds.z80_pc);
    s.z80_bc = format!("{:04X}", ds.z80_bc);
    s.z80_de = format!("{:04X}", ds.z80_de);
    s.z80_hl = format!("{:04X}", ds.z80_hl);

    // Log: last N event_log lines into the [log] Vec<String>.
    const LOG_TAIL: usize = 200;
    s.event_lines.clear();
    let start = ds.event_log.len().saturating_sub(LOG_TAIL);
    s.event_lines.extend(ds.event_log.iter().skip(start).cloned());

    // Audio display fields.
    s.volume_text = format!("{:.0}%", s.volume * 100.0);
    s.sample_rate = format!("{:.0}", audio.sample_rate);
}

// ─── Window title ────────────────────────────────────────────────────────────

fn update_title(emu: NonSend<Emu>, mut windows: Query<&mut Window>) {
    if emu.0.frame_count % 60 != 0 { return; }
    if let Ok(mut win) = windows.single_mut() {
        let fc  = emu.0.frame_count;
        let fps = emu.0.fps();
        win.title = match emu.0.framebuffer() {
            Some((_, w, h, _, fmt)) =>
                format!("RustRetro | frame:{fc} | {w}×{h} fmt={fmt} @ {fps:.0}fps"),
            None => format!("RustRetro | frame:{fc} | {fps:.0}fps"),
        };
    }
}
