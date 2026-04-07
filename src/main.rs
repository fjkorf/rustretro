mod frontend;
mod libretro;
mod sdl_interface;

use anyhow::Result;
use clap::Parser;
use frontend::Frontend;
use std::path::PathBuf;

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
}

fn main() -> Result<()> {
    // Initialize global environment strings
    frontend::initialize_env_strings();
    
    let args = Args::parse();

    // Validate inputs
    if !std::path::Path::new(&args.core).exists() {
        anyhow::bail!("Core file not found: {}", args.core);
    }
    if !std::path::Path::new(&args.rom).exists() {
        anyhow::bail!("ROM file not found: {}", args.rom);
    }

    eprintln!("RustRetro - Lightweight libretro Frontend");
    eprintln!("Core: {}", args.core);
    eprintln!("ROM: {}", args.rom);
    eprintln!("Save directory: {}", args.save_dir.display());
    eprintln!("System directory: {}", args.system_dir.display());
    eprintln!("Window scale: {}x", args.scale);
    eprintln!("Audio: {}", if args.no_audio { "disabled" } else { "enabled" });
    eprintln!();

    let mut frontend = Frontend::new(
        &args.core,
        &args.rom,
        args.save_dir,
        args.system_dir,
        args.scale,
        args.fullscreen,
        !args.no_audio,
    )?;

    eprintln!("Starting emulation...\n");
    frontend.run()?;

    eprintln!("\nEmulation ended cleanly.");
    Ok(())
}
