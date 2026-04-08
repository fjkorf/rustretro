mod debug;
mod frontend;
mod libretro;
mod sdl_interface;

use anyhow::Result;
use clap::Parser;
use debug::{DebugState, SharedDebugState};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Parser, Debug)]
#[command(name = "RustRetro")]
#[command(about = "Lightweight libretro frontend in Rust", long_about = None)]
struct Args {
    /// Path to the libretro core dynamic library (.so/.dll/.dylib)
    #[arg(long, value_name = "PATH")]
    core: String,

    /// Path to the game ROM or content file
    #[arg(long, value_name = "PATH")]
    rom: String,

    /// Start in fullscreen mode
    #[arg(long)]
    fullscreen: bool,

    /// Directory for save files and save states
    #[arg(long, value_name = "PATH", default_value = ".")]
    save_dir: PathBuf,

    /// Directory for BIOS/system files
    #[arg(long, value_name = "PATH", default_value = ".")]
    system_dir: PathBuf,

    /// Initial window scale factor
    #[arg(long, value_name = "FACTOR", default_value = "3")]
    scale: u32,

    /// Disable audio output
    #[arg(long)]
    no_audio: bool,

    /// Open debug window immediately on start
    #[arg(long)]
    debug: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();

    if !std::path::Path::new(&args.core).exists() {
        anyhow::bail!("Core file not found: {}", args.core);
    }
    if !std::path::Path::new(&args.rom).exists() {
        anyhow::bail!("ROM file not found: {}", args.rom);
    }

    eprintln!("RustRetro - Lightweight libretro Frontend");
    eprintln!("Core: {}", args.core);
    eprintln!("ROM:  {}", args.rom);
    eprintln!("Press F12 in-game to open the debug window.");
    eprintln!();

    // Shared state between emulation thread and debug window (main thread).
    let debug_state: SharedDebugState = Arc::new(Mutex::new(DebugState::new()));

    // Emulation runs in a background thread so the main thread can own the
    // eframe event loop (macOS requires AppKit/NSApp on the main thread).
    let emu_state = Arc::clone(&debug_state);
    let core = args.core.clone();
    let rom  = args.rom.clone();
    let save_dir   = args.save_dir.clone();
    let system_dir = args.system_dir.clone();
    let scale      = args.scale;
    let fullscreen = args.fullscreen;
    let no_audio   = args.no_audio;

    let emu_thread = std::thread::spawn(move || {
        let mut frontend = match frontend::Frontend::new(
            &core, &rom, save_dir, system_dir, scale, fullscreen, !no_audio, emu_state,
        ) {
            Ok(f) => f,
            Err(e) => { eprintln!("Frontend init error: {e}"); return; }
        };
        eprintln!("Starting emulation...");
        if let Err(e) = frontend.run() {
            eprintln!("Emulation error: {e}");
        }
        eprintln!("Emulation ended cleanly.");
    });

    // If --debug flag passed, open immediately; otherwise wait for F12 signal.
    if args.debug {
        debug_state.lock().unwrap().debug_open = true;
    }

    // Block main thread until debug window is requested, then run eframe.
    // Poll in a tight loop until the emulation thread signals debug_open or exits.
    loop {
        // Check if emulation thread finished (user closed SDL window)
        if emu_thread.is_finished() {
            break;
        }

        let open = debug_state.lock().map(|s| s.debug_open).unwrap_or(false);
        if open {
            // eframe must run on the main thread on macOS
            debug::window::run_main_thread(Arc::clone(&debug_state));
            // After debug window closes, clear the flag so it can reopen
            if let Ok(mut s) = debug_state.lock() {
                s.debug_open = false;
            }
        }

        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    let _ = emu_thread.join();
    Ok(())
}
