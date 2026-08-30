mod audio;
mod core_log;
mod profile;
mod debug;
mod frontend;
mod libretro;
mod litui_pages;
mod lua_engine;
mod macros;
mod record;
mod mcp;
mod shadow_runner;
mod training;
mod input_config;
mod gate;
mod hunt;
mod playback;

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
use litui_pages::TutorialPages;
use lua_engine::LuaEngine;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

// ─── CLI ─────────────────────────────────────────────────────────────────────

#[derive(Parser, Debug)]
#[command(name = "RustRetro", about = "Lightweight libretro frontend in Rust")]
struct Args {
    #[arg(long, value_name = "PATH")] core: String,
    #[arg(long, value_name = "PATH")] rom: String,
    /// Game profile directory or path with optional port selector.
    /// Formats: `library/asurabld` (a directory), or `library/mk2/genesis`
    /// (a directory with optional `/port_name` suffix for multi-port games).
    /// Each path loads family.json from its directory plus the matching port
    /// profile (see docs/game-profiles.md). Loaded once at startup before
    /// anything else touches game-specific memory knowledge.
    #[arg(long, value_name = "PATH", default_value = "library/asurabld")] game: String,
    #[arg(long)] fullscreen: bool,
    #[arg(long, value_name = "PATH", default_value = ".")] save_dir: PathBuf,
    #[arg(long, value_name = "PATH", default_value = ".")] system_dir: PathBuf,
    #[arg(long, value_name = "FACTOR", default_value = "3")] scale: u32,
    #[arg(long)] no_audio: bool,
    #[arg(long)] debug: bool,
    /// Optional Lua overlay script (loaded once at startup).
    #[arg(long, value_name = "PATH")] script: Option<PathBuf>,
    /// Expose the running app to a Claude session via an MCP server (AI Wave 1).
    #[arg(long)] mcp: bool,
    /// TCP port for the MCP server (only used with --mcp).
    #[arg(long, value_name = "N", default_value = "4000")] mcp_port: u16,
    /// Run with no window — emulator + MCP server only (for AI/agent-driven sessions). Implies --mcp.
    #[arg(long)] headless: bool,
    /// Headless wall-clock pacing multiplier (ignored in windowed mode, which
    /// paces through Bevy/vsync instead). 1.0 (default) = real time at the
    /// core's reported fps; 2.0 = double speed; 0 = uncapped, run as fast as
    /// possible. Use for interactive MCP probing (menu nav, timed input
    /// sequences) that needs a trustworthy clock.
    #[arg(long, value_name = "MULT", default_value = "1.0")] pace: f64,
    /// Busmap sidecar of bus windows to snapshot via the core's exported CPU
    /// bus API (Sek bridge; see library/asurabld/asurabld.busmap.json).
    /// Defaults to <save-dir>/<rom>.busmap.json when present.
    #[arg(long, value_name = "PATH")] bus_map: Option<PathBuf>,
    /// Record a per-frame trace (actor structs + P1/P2 input + controllable
    /// flag) as JSONL to this path — training data for the shadow project
    /// (schema: shadow/SPEC.md). Needs a Work RAM bus window (--bus-map).
    #[arg(long, value_name = "PATH")] record: Option<PathBuf>,
    /// Log raw gamepad button names + stick values to stderr whenever they
    /// change ("[pad] …") — for calibrating unmapped controllers.
    #[arg(long)] pad_debug: bool,
    /// Training mode (shadow PLAN Wave 2b): credits auto-topped-up, round
    /// timer held, health refilled before KO, dummy presets on port 1.
    /// Hotkeys: F1 cycle dummy, F2 reset positions, F3 toggle refill,
    /// F4 finish round.
    #[arg(long)] training: bool,
    /// Shadow bot: a fitted kNN model directory (shadow/models/<name>/ with
    /// cases.npz + meta.json). Loads at startup and drives controller port 1
    /// (P2) in-process — no shadow/play.py needed. Shift+F5 toggles at runtime.
    #[arg(long, value_name = "PATH")] shadow: Option<PathBuf>,
    /// Input mapping file (see src/input_config.rs). Default search:
    /// <save-dir>/<rom>.keymap.json, then <save-dir>/keymap.json, then the
    /// built-in maps.
    #[arg(long, value_name = "PATH")] keymap: Option<PathBuf>,
    /// Print the active input mapping as JSON to stdout and exit — the
    /// starting point for a hand-edited keymap file.
    #[arg(long)] dump_keymap: bool,
    /// Interactive controller calibration: prompts on stderr for each game
    /// action, captures the next gamepad button press, writes
    /// <save-dir>/keymap.json and exits. Esc skips a step.
    #[arg(long)] calibrate: bool,
    /// Start with audio muted (the stream still runs; unmute live from the
    /// debugger's audio controls). --no-audio disables audio entirely.
    #[arg(long)] mute: bool,
    /// Load a save state right after boot: a slot number 1-9 (resolved to
    /// <save-dir>/<rom>.state<N>) or an explicit state-file path. Applied on
    /// the first emulated frame (saving is interactive: F6/Shift+F6 or the MCP
    /// save_state tool).
    #[arg(long, value_name = "PATH_OR_SLOT")] load_state: Option<String>,
}

/// Parse the --load-state argument: a bare 1-9 selects a slot; anything else
/// is an explicit state-file path.
fn parse_load_state(spec: &str) -> debug::StateOp {
    match spec.trim().parse::<u8>() {
        Ok(n @ 1..=9) => debug::StateOp::LoadSlot(n),
        _ => debug::StateOp::Load(PathBuf::from(spec)),
    }
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

/// --pad-debug: log raw gamepad state changes from read_input.
#[derive(Resource)]
struct PadDebug(bool);

// ─── Entry point ─────────────────────────────────────────────────────────────

fn main() -> Result<()> {
    let mut args = Args::parse();

    // Headless without MCP is pointless (no window AND no agent channel). Auto-
    // enable MCP so `--headless` alone is sufficient for an agent-driven session.
    if args.headless && !args.mcp {
        args.mcp = true;
        eprintln!("[headless] --headless implies --mcp; enabling MCP server.");
    }

    if !std::path::Path::new(&args.core).exists() { anyhow::bail!("Core not found: {}", args.core); }
    if !std::path::Path::new(&args.rom).exists()  { anyhow::bail!("ROM not found: {}", args.rom); }

    // Load the game profile (docs/game-profiles.md) before anything else reads
    // game-specific memory knowledge — record/training/frontend all resolve
    // addresses via `profile::current()` from this point on. A malformed or
    // missing profile is a hard startup error, same posture as --shadow.
    let game_dir = PathBuf::from(&args.game);
    profile::init(&game_dir).map_err(|e| {
        anyhow::anyhow!("--game {}: failed to load game profile: {e}", game_dir.display())
    })?;
    eprintln!("[profile] loaded {} ({})", game_dir.display(), profile::current().port.port);
    for pin in &profile::current().port.pins {
        eprintln!(
            "[profile] pin: {} = {} (held for the session, asserted 1/s)",
            pin.global, pin.value
        );
    }

    eprintln!("RustRetro — Bevy libretro frontend");
    eprintln!("Core: {}", args.core);
    eprintln!("ROM:  {}", args.rom);
    for (group, binds) in KEYBINDINGS {
        let line: Vec<String> = binds.iter().map(|(k, a)| format!("{k} {a}")).collect();
        eprintln!("{group}: {}", line.join(" · "));
    }

    // Input mapping: resolve config, honor --dump-keymap before anything else.
    let keymap_cfg = input_config::InputConfig::load(&args.keymap, &args.save_dir, &args.rom);
    if args.dump_keymap {
        println!("{}", serde_json::to_string_pretty(&keymap_cfg).unwrap());
        return Ok(());
    }

    let debug_state: SharedDebugState = Arc::new(Mutex::new(DebugState::new()));
    // The Help panel shows the ACTIVE bindings (keymap.json / --keymap /
    // default), not a hardcoded copy — publish the resolved summary once.
    debug_state.lock().unwrap().keymap_lines = input_config::summary(&keymap_cfg);

    let mut frontend = Frontend::new(
        &args.core, &args.rom,
        args.save_dir.clone(), args.system_dir.clone(),
        Arc::clone(&debug_state),
        args.bus_map.clone(),
    )?;

    // Prerequisite advisory: this profile declares it needs memory regions
    // (fighter blocks / globals live in Work RAM) but none are installed yet.
    // Not fatal — a bus window can still arrive later via --bus-map or a Lua
    // script — but a silent gate-always-closed session is a confusing failure
    // mode, so warn loudly and by name.
    if profile::current().port.requires.memory_regions {
        let no_regions = debug_state
            .lock()
            .map(|ds| ds.memory_regions.is_empty())
            .unwrap_or(false);
        if no_regions {
            eprintln!(
                "[profile] WARNING: profile '{}' requires memory_regions but none are \
                 installed yet (no --bus-map / core memory map found) — reads will return 0 \
                 and the controllable gate will stay closed until a bus window is installed.",
                profile::current().port.port
            );
        }
    }

    // Enable per-frame trace recording if requested (both GUI and headless).
    if let Some(rec_path) = args.record.clone() {
        frontend.set_recorder(rec_path, None);
    }

    // --shadow: load the kNN shadow-bot model and start it enabled (Shift+F5
    // toggles later; in headless there is no keyboard, so startup-enabled is
    // the only sensible default). A malformed model is a hard startup error —
    // a silently absent shadow would be worse than a loud one.
    if let Some(dir) = &args.shadow {
        let runner = shadow_runner::ShadowRunner::load(dir)
            .map_err(|e| anyhow::anyhow!("--shadow {}: {e}", dir.display()))?;
        frontend.set_shadow(runner);
    }

    // --load-state: queue the load now; the Frontend drains it at the safe
    // point right after the FIRST core.run (the core has warmed up one frame
    // by then — retro_unserialize before any retro_run is not reliable across
    // cores).
    if let Some(spec) = &args.load_state {
        let op = parse_load_state(spec);
        debug_state.lock().unwrap().pending_state_op = Some(op);
        eprintln!("[state] --load-state {spec}: queued, applied after the first frame");
    }

    let w = frontend.video_width().max(320) * args.scale;
    let h = frontend.video_height().max(240) * args.scale;
    // Core audio rate for the resampler (e.g. fbalpha2012: 32040 Hz).
    let core_sample_rate = frontend.sample_rate();

    if args.debug { debug_state.lock().unwrap().debug_open = true; }
    if args.training {
        let mut ds = debug_state.lock().unwrap();
        ds.training.enabled = true;
        ds.training.refill = true;
        // Training sessions exist to poke RAM: arm the Lua write bindings too
        // (memory.writebyte/writeword). Outside --training they stay locked
        // until the MCP enable_writes tool arms them.
        ds.lua_writes_enabled = true;
        eprintln!(
            "[training] mode ON — credits auto, timer held, health refill, Lua writes armed. \
             F1 cycle dummy, F2 reset positions, F3 toggle refill, F4 finish round."
        );
    }

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
        .insert_resource(AudioRes({
            let mut a = AudioOutput::new(!args.no_audio, core_sample_rate);
            if args.mute {
                a.set_mute(true);
                eprintln!("[audio] starting muted (--mute)");
            }
            a
        }))
        .insert_resource(WindowScale(args.scale))
        .insert_resource(PadDebug(args.pad_debug))
        .insert_resource(keymap_cfg)
        .insert_resource(CalibrateState::new(args.calibrate, &args.save_dir))
        .insert_resource(DebugOverlay(debug::window::DebugApp::new(debug_state)))
        .insert_non_send_resource(LuaRes(lua_engine))
        .insert_resource(ScriptPanel::new())
        .insert_resource(debug::panels::controls::ControlsPanel::new())
        .init_resource::<TutorialPages>()
        .add_systems(Startup, setup)
        .add_systems(Update, (calibrate_wizard, read_input, run_emulation, hunt_sample, input_log_sample, run_scripts, drain_lua_requests, sync_video, queue_audio, update_title).chain())
        .add_systems(EguiPrimaryContextPass, (show_debug, show_script_panel, debug::panels::controls::show_controls_panel, show_tutorial_pages))
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
    lua_engine: LuaEngine,
    debug_state: SharedDebugState,
    args: &Args,
) -> Result<()> {
    use std::time::{Duration, Instant};

    let fps = frontend.fps().max(1.0);
    let pace = args.pace;
    // --pace 0 (or negative) means uncapped: run flat-out, no sleeping at all.
    let uncapped = pace <= 0.0;
    // Effective frame period: at pace 2.0 we want to *emit* frames twice as
    // fast (half the wall-clock period), so divide by pace, not multiply.
    let frame_dur = if uncapped { Duration::ZERO } else { Duration::from_secs_f64(1.0 / (fps * pace)) };

    eprintln!(
        "[headless] running {fps:.0} fps × pace {pace} — {}. MCP on http://127.0.0.1:{}/mcp. Ctrl-C to stop.",
        if uncapped { "uncapped".to_string() } else { format!("{:.1} fps wall-clock", fps * pace) },
        args.mcp_port
    );

    // Drift-corrected pacing: schedule each frame off a fixed `sched_start`
    // anchor plus its frame index, rather than resetting a per-frame `start`
    // timer and sleeping only *that frame's own* leftover budget (the old
    // code: `start = Instant::now()`, then `sleep(frame_dur - start.elapsed())`
    // at the bottom). That per-frame-reset scheme has no memory across
    // frames: whenever one frame's own work (a lua callback, an MCP
    // round-trip, a bus-window snapshot — CLAUDE.md notes those cost ~10ms)
    // eats into or exceeds its 16.7ms budget, `checked_sub` comes back None,
    // the sleep is skipped entirely for that frame, and the shortfall is
    // simply forgotten — nothing later slows down to repay it. There was
    // also no `--pace` knob and no minimum-sleep guard, so the only actual
    // behaviors reachable were "best-effort sleep-to-16.7ms" or nothing,
    // and any run with occasional over-budget frames drifts fast of nominal
    // with no correction — consistent with the ~1.5-2x-realtime effective
    // speed observed live. Anchoring each frame's deadline to
    // `sched_start + frame_index * frame_dur` makes the target absolute:
    // a late frame just means the next sleep is shorter (or skipped), and
    // the schedule never drifts further than one frame's worth of slack.
    let sched_start = Instant::now();
    let mut frame_index: u64 = 0;

    loop {
        // (a0) Fold any MCP-injected controller input for this frame (headless has
        //      no keyboard) so an agent can drive menus/moves via press_buttons.
        //      Both ports: P1 for the agent/user, P2 for the shadow bot / dummy.
        //      Bumps `fold_generation` (+ notifies `frame_cv`) so `run_frames`
        //      can confirm a fold observed newly-set held masks before it arms
        //      a batch — see `RetroMcpServer::run_frames`'s doc for the race
        //      this closes. Runs every iteration regardless of `paused`.
        {
            let (injected, injected2) = match debug_state.lock() {
                Ok(mut ds) => {
                    let f = (ds.take_injected_input(), ds.take_injected_input2());
                    ds.fold_generation = ds.fold_generation.wrapping_add(1);
                    ds.frame_cv.notify_all();
                    f
                }
                Err(_) => ([false; 12], [false; 12]),
            };
            frontend.set_input(injected);
            frontend.set_input2(injected2);
        }

        // (a) Tick the core one frame. run_frame() honours pause/step/trigger
        //     flags in DebugState internally, so MCP pause/resume/step work.
        let _ = frontend.run_frame();

        // (a1) Signal hunt: snapshot the scoped hunt region into the ring
        //      (docs/signal-hunt.md §3). Before the Lua callbacks so a
        //      `hunt.mark()` from a per-frame script marks a frame that already
        //      has a snapshot. No-ops when the frame counter didn't advance.
        hunt::sample(&debug_state);

        // (a1b) Input log (task A1): one frame-exact sample of both ports'
        //       already-folded button masks — same hook point as hunt::sample
        //       (right after run_frame, before pause/step can re-offer this
        //       frame), so pause/resume dedup identically. See
        //       debug/panels/input_log.rs's module doc for the fold and
        //       save-state-load contract.
        debug::panels::input_log::sample(&debug_state);

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

        // (d) Frame pacing: sleep until this frame's absolute deadline. When
        //     the core is paused, run_frame() returned early (cheap), so this
        //     still paces the loop at ~fps × pace — the agent keeps a
        //     responsive run_lua / memory-read channel while paused.
        frame_index += 1;
        if !uncapped {
            let deadline = sched_start + frame_dur.mul_f64(frame_index as f64);
            let now = Instant::now();
            if let Some(rem) = deadline.checked_duration_since(now) {
                // Sub-millisecond sleeps are unreliable and waste OS calls at
                // high --pace multipliers; only sleep once we're ahead by
                // more than 1ms, otherwise fall straight into the next frame
                // (the deadline stays absolute, so this doesn't accumulate).
                if rem > Duration::from_millis(1) {
                    std::thread::sleep(rem);
                }
            }
            // else: we're behind schedule — run flat-out to catch back up;
            // the fixed anchor means we can never fall more than one frame's
            // worth of slack behind nominal pace.
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

/// Interactive controller calibration (--calibrate): prompts through the
/// wizard steps on stderr, captures one gamepad button per game action,
/// writes keymap.json, exits. Eliminates every "which physical button was
/// that" ambiguity — the user answers by pressing, never by describing.
#[derive(Resource)]
struct CalibrateState {
    active: bool,
    step: usize,
    announced: bool,
    captured: std::collections::BTreeMap<GamepadButton, Vec<input_config::RetroButton>>,
    out_path: PathBuf,
}

impl CalibrateState {
    fn new(active: bool, save_dir: &std::path::Path) -> Self {
        CalibrateState {
            active,
            step: 0,
            announced: false,
            captured: Default::default(),
            out_path: input_config::InputConfig::global_path(save_dir),
        }
    }
}

/// (prompt, RETRO bits the captured button will emit) — generated from the
/// action vocabulary (`input_config::action_rows`, docs/game-profiles.md
/// "Controls contract") instead of hardcoded Asura Blade semantics, so a new
/// game profile gets a working wizard for free. Descriptors aren't available
/// pre-boot (the wizard runs at startup before the core sends any), so we
/// pass an all-`None` descriptor table — `profile::current()` (profile::init
/// already ran in `main`) supplies every name the wizard needs. Direction
/// rows are skipped: the lever/dpad passthrough handles those, as before.
fn calibrate_steps() -> Vec<(String, Vec<input_config::RetroButton>)> {
    use input_config::RetroButton;
    let descriptors: [[Option<String>; 12]; 2] = Default::default();
    input_config::action_rows(0, &descriptors)
        .into_iter()
        .filter(|row| {
            !(row.bits.len() == 1
                && matches!(
                    row.bits[0],
                    RetroButton::Up | RetroButton::Down | RetroButton::Left | RetroButton::Right
                ))
        })
        .map(|row| (row.name.to_uppercase(), row.bits))
        .collect()
}

#[cfg(test)]
mod calibrate_steps_tests {
    use super::calibrate_steps;
    use crate::input_config::RetroButton::*;

    /// Resolver order is Start, Coin, then attack classes in family order
    /// (family.json: Light, Medium, Heavy, Launcher, Toss) — differs from
    /// the old hardcoded wizard's ORDER (attacks first), but the SET of
    /// (name, bits) pairs must cover the exact same bits.
    #[test]
    fn asurabld_generated_steps_match_legacy_bit_set() {
        crate::profile::init_for_tests();
        let steps = calibrate_steps();
        let got: Vec<(&str, Vec<crate::input_config::RetroButton>)> =
            steps.iter().map(|(n, b)| (n.as_str(), b.clone())).collect();
        assert_eq!(
            got,
            vec![
                ("START", vec![Start]),
                ("COIN", vec![Select]),
                ("LIGHT", vec![B]),
                ("MEDIUM", vec![A]),
                ("HEAVY", vec![Y]),
                ("LAUNCHER", vec![B, A]),
                ("TOSS", vec![B, A, Y]),
            ]
        );
        // Directions are never steps — the lever/dpad passthrough covers them.
        for (name, _) in &steps {
            assert!(!matches!(name.as_str(), "UP" | "DOWN" | "LEFT" | "RIGHT"));
        }
    }
}

fn calibrate_wizard(
    mut cal: ResMut<CalibrateState>,
    pads: Query<&Gamepad>,
    keys: Res<ButtonInput<KeyCode>>,
    mut cfg: ResMut<input_config::InputConfig>,
) {
    if !cal.active {
        return;
    }
    let steps = calibrate_steps();
    if cal.step >= steps.len() {
        // Finish: keep keyboard maps + deadzone from the active config, use
        // the captured gamepad map (plus lever passthrough) on both ports.
        use input_config::RetroButton::{Down, Left, Right, Up};
        let mut gp = cal.captured.clone();
        for (b, d) in [
            (GamepadButton::DPadUp, Up),
            (GamepadButton::DPadDown, Down),
            (GamepadButton::DPadLeft, Left),
            (GamepadButton::DPadRight, Right),
        ] {
            gp.entry(b).or_insert_with(|| vec![d]);
        }
        for port in cfg.ports.iter_mut() {
            port.gamepad = gp.iter().map(|(b, v)| (*b, input_config::Chord(v.clone()))).collect();
        }
        let json = serde_json::to_string_pretty(&*cfg).unwrap();
        match std::fs::write(&cal.out_path, &json) {
            Ok(()) => eprintln!(
                "[calibrate] wrote {} — relaunch (without --calibrate) to play",
                cal.out_path.display()
            ),
            Err(e) => eprintln!("[calibrate] FAILED to write {}: {e}", cal.out_path.display()),
        }
        std::process::exit(0);
    }
    let (prompt, bits) = &steps[cal.step];
    if !cal.announced {
        eprintln!(
            "[calibrate] step {}/{}: press the button for {prompt}  (Esc = skip)",
            cal.step + 1,
            steps.len()
        );
        cal.announced = true;
    }
    if keys.just_pressed(KeyCode::Escape) {
        eprintln!("[calibrate] skipped");
        cal.step += 1;
        cal.announced = false;
        return;
    }
    let Some(pad) = pads.iter().next() else { return };
    let fresh: Option<GamepadButton> = pad
        .get_just_pressed()
        .copied()
        .find(|b| {
            !matches!(
                b,
                GamepadButton::DPadUp
                    | GamepadButton::DPadDown
                    | GamepadButton::DPadLeft
                    | GamepadButton::DPadRight
            )
        });
    if let Some(btn) = fresh {
        if cal.captured.contains_key(&btn) {
            eprintln!("[calibrate] {btn:?} is already assigned — press a different button");
            return;
        }
        eprintln!("[calibrate]   {prompt} = {btn:?}");
        let bits = bits.clone();
        cal.captured.insert(btn, bits);
        cal.step += 1;
        cal.announced = false;
    }
}

/// The single source of truth for hotkey documentation, grouped for display.
/// Rendered by the Help panel (❓ Help → Keybindings) and printed at startup —
/// when you add or change a hotkey in `read_input` below, update this table
/// in the same commit (there is no third copy to keep in sync).
pub const KEYBINDINGS: &[(&str, &[(&str, &str)])] = &[
    ("Debugger", &[
        ("F12", "toggle debug overlay"),
        ("Space", "pause / unpause emulation"),
        ("B", "capture bookmark"),
        ("F8", "tutorials window"),
        ("F10", "Lua script panel"),
        ("F11", "controls panel (view + rebind)"),
    ]),
    ("Save states", &[
        ("F6", "save state → slot 1 (Shift: slot 2)"),
        ("F7", "load state ← slot 1 (Shift: slot 2)"),
        ("--load-state N|path", "load at launch, after the first frame"),
    ]),
    ("Training & shadow", &[
        ("F5", "toggle training mode (credits auto, timer held, refill)"),
        ("F1", "cycle dummy: Free → Stand → Crouch → Jump → Block"),
        ("F2", "reset positions"),
        ("F3", "toggle health refill"),
        ("F4", "finish round"),
        ("Shift+F5", "toggle shadow bot (needs --shadow)"),
    ]),
    ("Signal hunt", &[
        ("F9", "mark this frame as an 'event' (🔍 Signal Hunt panel)"),
        ("Shift+F9", "mark this frame as a 'control' (a near-miss)"),
    ]),
];

fn read_input(
    keys: Res<ButtonInput<KeyCode>>,
    pads: Query<(Entity, &Gamepad, Option<&Name>)>,
    mut emu: NonSendMut<Emu>,
    debug_state: Res<DebugStateRes>,
    mut script_panel: ResMut<ScriptPanel>,
    mut tutorials: ResMut<TutorialPages>,
    pad_debug: Res<PadDebug>,
    cfg: Res<input_config::InputConfig>,
    mut last_pad_dbg: Local<String>,
    // `Option` because the Controls panel resource is inserted by the App
    // builder (outside this function's ownership, per the Controls-phase
    // task split) — this stays a no-op interlock until that insert_resource
    // lands, then picks it up live with no further change here.
    mut controls_panel: Option<ResMut<crate::debug::panels::controls::ControlsPanel>>,
) {
    use KeyCode::*;
    // Capture-mode interlock (Controls panel, docs/game-profiles.md): while a
    // rebind capture is armed, don't fold keyboard/gamepad into game input —
    // a capture keypress must not also punch in-game. Hotkeys (F1-F12) below
    // still run.
    let capturing = controls_panel.as_deref().is_some_and(|p| p.capturing());
    // Keyboard bindings per port, from the active keymap config.
    let mut bits = if capturing {
        [false; 12]
    } else {
        input_config::key_bits(|k| keys.pressed(k), &cfg.ports[0])
    };
    let mut bits2 = if capturing {
        [false; 12]
    } else if cfg.ports.len() > 1 {
        input_config::key_bits(|k| keys.pressed(k), &cfg.ports[1])
    } else {
        [false; 12]
    };
    // Fold physical gamepads: lowest entity id → slot 0, routed to ports via
    // cfg.pad_order (keyboard still live — OR, not replace). Replugging
    // mid-session can reorder pads; pad_order [1,0] swaps two pads.
    let mut pad_list: Vec<_> = pads.iter().collect();
    pad_list.sort_by_key(|(e, _, _)| *e);
    if !capturing {
        for (slot, (_, pad, name)) in pad_list.iter().take(2).enumerate() {
            let port = cfg.pad_order.get(slot).copied().unwrap_or(slot);
            if port >= cfg.ports.len().min(2) {
                continue;
            }
            let gb = input_config::pad_bits_for_device(
                |b| pad.pressed(b),
                pad.left_stick(),
                &cfg.ports[port],
                cfg.stick_deadzone,
                name.map(|n| n.as_str()),
            );
            let target = if port == 0 { &mut bits } else { &mut bits2 };
            for i in 0..12 {
                target[i] |= gb[i];
            }
        }
    }
    // --pad-debug: log the raw pressed set + stick when it changes, so unmapped
    // controllers' button identities can be observed without a rebuild.
    if pad_debug.0 {
        if let Some((_, pad, _)) = pad_list.first() {
            let cur = format!(
                "{:?} stick={:.2},{:.2} dpad={:.2},{:.2}",
                pad.get_pressed().collect::<Vec<_>>(),
                pad.left_stick().x,
                pad.left_stick().y,
                pad.dpad().x,
                pad.dpad().y
            );
            if *last_pad_dbg != cur {
                eprintln!("[pad] {cur}");
                *last_pad_dbg = cur;
            }
        }
    }
    // OR in any MCP-injected input (press_buttons) so an agent — or the shadow bot
    // on P2 — can drive either port in windowed mode alongside the keyboard.
    // Bumps `fold_generation` (+ notifies `frame_cv`), same contract as the
    // headless loop's step (a0) — see `RetroMcpServer::run_frames`'s doc.
    if let Ok(mut ds) = debug_state.0.lock() {
        let injected = ds.take_injected_input();
        let injected2 = ds.take_injected_input2();
        ds.fold_generation = ds.fold_generation.wrapping_add(1);
        ds.frame_cv.notify_all();
        for i in 0..12 {
            bits[i] |= injected[i];
            bits2[i] |= injected2[i];
        }
        // Controls panel integration contract: mirror descriptors/save_dir/
        // rom_stem while we already hold the DebugState lock.
        if let Some(panel) = controls_panel.as_deref_mut() {
            panel.sync_descriptors(&ds);
        }
    }
    emu.0.set_input(bits);
    emu.0.set_input2(bits2);
    // F5 toggles training mode at runtime (equivalent to launching with
    // --training); Shift+F5 toggles the shadow bot (--shadow) instead. The
    // F1-F4 handlers below only respond while training is on.
    if keys.just_pressed(F5) && (keys.pressed(ShiftLeft) || keys.pressed(ShiftRight)) {
        emu.0.toggle_shadow();
    } else if keys.just_pressed(F5) {
        if let Ok(mut ds) = debug_state.0.lock() {
            ds.training.enabled = !ds.training.enabled;
            if ds.training.enabled {
                ds.training.refill = true;
                eprintln!(
                    "[training] ON — credits auto, timer held, health refill. \
                     F1 cycle dummy, F2 reset positions, F3 toggle refill, F4 finish round, F5 off."
                );
            } else {
                eprintln!("[training] OFF (frozen values release next frame)");
            }
        }
    }
    // Training-mode hotkeys (F5 or --training to enable).
    if keys.just_pressed(F1) || keys.just_pressed(F2) || keys.just_pressed(F3) || keys.just_pressed(F4)
    {
        if let Ok(mut ds) = debug_state.0.lock() {
            if !ds.training.enabled {
                eprintln!(
                    "[training] not enabled — press F5 or launch with --training \
                     (F1-F4 are training-mode keys)"
                );
            }
            if ds.training.enabled {
                use debug::DummyMode::*;
                if keys.just_pressed(F1) {
                    ds.training.dummy = match ds.training.dummy {
                        Free => Stand,
                        Stand => Crouch,
                        Crouch => Jump,
                        Jump => Block,
                        Block => BlockPunish,
                        BlockPunish => Free,
                    };
                    eprintln!("[training] dummy: {:?}", ds.training.dummy);
                }
                if keys.just_pressed(F2) {
                    ds.training.reset_positions = true;
                    eprintln!("[training] reset positions");
                }
                if keys.just_pressed(F3) {
                    ds.training.refill = !ds.training.refill;
                    eprintln!("[training] health refill: {}", ds.training.refill);
                }
                if keys.just_pressed(F4) {
                    ds.training.finish_round = true;
                    eprintln!("[training] finish round");
                }
            }
        }
    }
    // Save-state hotkeys: F6 save / F7 load, slot 1 (Shift → slot 2). The op is
    // queued here and performed by the emulation thread at its safe point.
    if keys.just_pressed(F6) || keys.just_pressed(F7) {
        let slot = if keys.pressed(ShiftLeft) || keys.pressed(ShiftRight) { 2u8 } else { 1u8 };
        if let Ok(mut ds) = debug_state.0.lock() {
            if keys.just_pressed(F6) {
                ds.pending_state_op = Some(debug::StateOp::SaveSlot(slot));
                eprintln!("[state] F6: save state → slot {slot} queued");
            } else {
                ds.pending_state_op = Some(debug::StateOp::LoadSlot(slot));
                eprintln!("[state] F7: load state ← slot {slot} queued");
            }
        }
    }
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
    if keys.just_pressed(F8) {
        tutorials.open = !tutorials.open;
    }
    if keys.just_pressed(F11) {
        if let Some(panel) = controls_panel.as_deref_mut() {
            panel.open = !panel.open;
        }
    }
    // Signal hunt (docs/signal-hunt.md §2): mark THIS frame without leaving the
    // game. Marking has to be possible at the instant you SEE the event — a
    // mouse trip to the panel is several frames of drift — so the hotkey is the
    // primary marking surface during a live hunt.
    if keys.just_pressed(F9) {
        let control = keys.pressed(ShiftLeft) || keys.pressed(ShiftRight);
        let label = if control { "control" } else { "event" };
        let msg = {
            let ds = debug_state.0.lock().unwrap();
            hunt::mark_with(&ds, label)
        };
        match msg {
            Ok(m) => eprintln!("[hunt] {m}"),
            Err(e) => eprintln!("[hunt] mark failed: {e}"),
        }
    }
}

// ─── Emulation ───────────────────────────────────────────────────────────────

/// Pace emulation to the core's fps regardless of display refresh. Bevy's
/// Update schedule runs at the panel rate (120 Hz on ProMotion), and calling
/// run_frame unconditionally ran the game at 2x. Accumulate real time and run
/// whole emulated frames as the budget allows; cap catch-up bursts so a long
/// stall doesn't fast-forward.
fn run_emulation(
    mut emu: NonSendMut<Emu>,
    mut acc: Local<Option<(std::time::Instant, f64)>>,
) {
    const MAX_BURST: u32 = 3;
    let fps = emu.0.fps().max(1.0);
    let frame = 1.0 / fps;
    let now = std::time::Instant::now();
    let (last, mut budget) = acc.unwrap_or((now, frame));
    budget += now.duration_since(last).as_secs_f64();
    let mut ran = 0;
    while budget >= frame && ran < MAX_BURST {
        let _ = emu.0.run_frame();
        budget -= frame;
        ran += 1;
    }
    budget = budget.min(frame * MAX_BURST as f64);
    *acc = Some((now, budget));
}

/// Signal hunt (docs/signal-hunt.md §3): push one snapshot of the scoped hunt
/// region into the ring, right after the emulator ran. Sits between
/// `run_emulation` and `run_scripts` so a Lua `hunt.mark()` in a per-frame
/// callback marks a frame whose snapshot already exists. `hunt::sample` no-ops
/// when the frame counter did not advance (paused), so this is free while
/// stepping or idling.
fn hunt_sample(debug_state: Res<DebugStateRes>) {
    hunt::sample(&debug_state.0);
}

/// Input log (task A1): mirrors `hunt_sample`'s hook point exactly — see
/// `debug/panels/input_log.rs`'s module doc.
fn input_log_sample(debug_state: Res<DebugStateRes>) {
    debug::panels::input_log::sample(&debug_state.0);
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
    // Track the core rate each frame (SET_SYSTEM_AV_INFO can change it).
    audio.0.set_input_rate(emu.0.sample_rate());
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

// ─── tutorial pages (litui) ─────────────────────────────────────────────────

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


// ─── Window title ────────────────────────────────────────────────────────────

#[cfg(test)]
mod load_state_arg_tests {
    use super::parse_load_state;
    use crate::debug::StateOp;
    use std::path::PathBuf;

    #[test]
    fn bare_digit_is_slot_anything_else_is_path() {
        assert_eq!(parse_load_state("1"), StateOp::LoadSlot(1));
        assert_eq!(parse_load_state(" 9 "), StateOp::LoadSlot(9));
        // 0 and >9 are not valid slots — treated as (odd) paths, not slots.
        assert_eq!(parse_load_state("0"), StateOp::Load(PathBuf::from("0")));
        assert_eq!(parse_load_state("12"), StateOp::Load(PathBuf::from("12")));
        assert_eq!(
            parse_load_state("/tmp/foo.state1"),
            StateOp::Load(PathBuf::from("/tmp/foo.state1"))
        );
    }
}

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
