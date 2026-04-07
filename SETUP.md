# RustRetro Setup & Usage Guide

## Quick Start

### 1. Install SDL2

**macOS:**
```bash
brew install sdl2
```

**Ubuntu/Debian:**
```bash
sudo apt-get update
sudo apt-get install libsdl2-dev
```

**Fedora/RHEL:**
```bash
sudo dnf install SDL2-devel
```

**Windows (MSVC):**
Download SDL2 development libraries from https://www.libsdl.org/download-2.0.php and add to your system PATH.

### 2. Build RustRetro

```bash
cd /path/to/rustretro
cargo build --release
```

The binary will be at `target/release/rustretro` (or `.exe` on Windows).

### 3. Obtain a Libretro Core

Download a libretro core from https://buildbot.libretro.com/nightly/

Examples:
- **SNES**: `snes9x_libretro.so`
- **Genesis**: `genesis_plus_gx_libretro.so`
- **Game Boy**: `mgba_libretro.so`
- **NES**: `fceumm_libretro.so`

### 4. Prepare ROMs & BIOS Files

```bash
mkdir -p ~/Games/Cores
mkdir -p ~/Games/ROMs
mkdir -p ~/Games/Saves
mkdir -p ~/Games/BIOS

# Place cores in Cores directory
# Place ROMs in ROMs directory
# Place any required BIOS files in BIOS directory (system-dir)
```

### 5. Run a Game

```bash
./target/release/rustretro \
  --core ~/Games/Cores/snes9x_libretro.so \
  --rom ~/Games/ROMs/mario.sfc \
  --save-dir ~/Games/Saves \
  --system-dir ~/Games/BIOS \
  --scale 2
```

Or with cargo:
```bash
cargo run --release -- \
  --core ~/Games/Cores/snes9x_libretro.so \
  --rom ~/Games/ROMs/mario.sfc
```

## Controls

| Key | Action |
|-----|--------|
| Arrow Keys | D-Pad (Up/Down/Left/Right) |
| Z | B Button |
| X | A Button |
| A | Y Button |
| S | X Button |
| Enter | Start Button |
| Left Shift | Select Button |
| Q | L Button |
| W | R Button |
| ESC or Click X | Quit |

## Creating a Shell Alias

Add to your `.bashrc`, `.zshrc`, or shell config:

```bash
alias rustretro='~/path/to/rustretro/target/release/rustretro'

# Usage:
rustretro --core ~/Games/Cores/snes9x_libretro.so --rom ~/Games/ROMs/game.sfc
```

## Troubleshooting

### "Core file not found"
- Verify the core path is correct: `ls -la /path/to/core.so`
- Use absolute paths instead of relative paths

### "ROM file not found"
- Check ROM file exists: `ls -la /path/to/rom.sfc`
- Make sure file extension is correct

### SDL2 compilation errors
- Ensure SDL2 development headers are installed
- Try reinstalling: `brew reinstall sdl2` (macOS) or `apt-get install --reinstall libsdl2-dev` (Linux)
- Check `sdl2-config` is in your PATH: `which sdl2-config`

### Core crashes or "API version mismatch"
- Ensure core is compatible with libretro API v1
- Some very old cores may not be compatible
- Try a newer version of the core from https://buildbot.libretro.com/

### No audio output
- This is expected - audio is not fully implemented yet
- Use `--no-audio` to suppress audio-related errors if any

### Window appears but no video
- This is expected in the current version - video rendering is stubbed
- This is a known limitation that needs SDL2 texture implementation

## Creating a Game Launcher Script

Save as `run-game.sh`:

```bash
#!/bin/bash

RUSTRETRO="$HOME/path/to/rustretro/target/release/rustretro"
CORES_DIR="$HOME/Games/Cores"
ROMS_DIR="$HOME/Games/ROMs"
SAVES_DIR="$HOME/Games/Saves"
BIOS_DIR="$HOME/Games/BIOS"

if [ $# -lt 2 ]; then
    echo "Usage: $0 <core_name> <rom_file>"
    echo "Example: $0 snes9x mario.sfc"
    exit 1
fi

CORE="$CORES_DIR/${1}_libretro.so"
ROM="$ROMS_DIR/$2"

if [ ! -f "$CORE" ]; then
    echo "Core not found: $CORE"
    exit 1
fi

if [ ! -f "$ROM" ]; then
    echo "ROM not found: $ROM"
    exit 1
fi

mkdir -p "$SAVES_DIR" "$BIOS_DIR"

$RUSTRETRO \
    --core "$CORE" \
    --rom "$ROM" \
    --save-dir "$SAVES_DIR" \
    --system-dir "$BIOS_DIR" \
    --scale 2
```

Usage:
```bash
chmod +x run-game.sh
./run-game.sh snes9x mario.sfc
```

## Performance Tips

1. **Use release builds**: Always use `--release` builds for better performance
2. **Lower scale**: Use `--scale 1` if experiencing lag
3. **Disable audio**: Use `--no-audio` if audio causes stuttering
4. **System resources**: Close other applications while emulating

## Supported Cores

RustRetro can work with any libretro core, including:

- **snes9x** - SNES emulation
- **genesis_plus_gx** - Sega Genesis/Mega Drive
- **mgba** - Game Boy Advance
- **fceumm** - NES
- **pcsx_rearmed** - PlayStation 1
- **mupen64plus** - Nintendo 64
- And many more from https://buildbot.libretro.com/

## Known Limitations

- ⚠️ **Video Output**: Framebuffer is captured but not rendered to window (implementation incomplete)
- ⚠️ **Audio**: Audio callback infrastructure exists but playback is basic
- ⚠️ **Single Player**: Only supports one controller
- ⚠️ **Save States**: Not implemented yet
- ⚠️ **Core Options**: Core configuration variables not supported

## Development

To contribute or modify the code:

```bash
# Clone/navigate to repository
cd /path/to/rustretro

# Build with optimizations
cargo build --release

# Run tests
cargo test

# Build documentation
cargo doc --open

# Check for issues
cargo clippy
```

### Project Structure

```
src/
├── main.rs           # CLI entry point
├── libretro.rs       # Libretro FFI bindings
├── sdl_interface.rs  # SDL2 wrapper
└── frontend.rs       # Main frontend logic
```

## License & Attribution

This is a learning/reference implementation of a libretro frontend.

## Resources

- [Libretro Documentation](https://docs.libretro.com/)
- [Libretro GitHub](https://github.com/libretro)
- [SDL2 Rust Docs](https://docs.rs/sdl2/)
- [Rust Book](https://doc.rust-lang.org/book/)
