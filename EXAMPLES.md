# RustRetro Usage Examples

## Basic Usage

### Minimal Example
```bash
./target/release/rustretro --core ./snes9x_libretro.so --rom ./game.sfc
```

### With Options
```bash
./target/release/rustretro \
  --core ./snes9x_libretro.so \
  --rom ./game.sfc \
  --scale 2 \
  --save-dir ./saves \
  --system-dir ./bios
```

### Fullscreen at 1x Scale
```bash
./target/release/rustretro \
  --core ./genesis_plus_gx_libretro.so \
  --rom ./sonic.gen \
  --fullscreen \
  --scale 1
```

### Disable Audio
```bash
./target/release/rustretro \
  --core ./mgba_libretro.so \
  --rom ./pokemon.gba \
  --no-audio
```

## Using Cargo

### Run Directly with Cargo
```bash
cargo run --release -- \
  --core /path/to/core.so \
  --rom /path/to/game.rom
```

### Help Message
```bash
cargo run -- --help
```

### Check Build
```bash
cargo build --release
```

## Directory Organization Examples

### Example 1: Organized Game Collection
```
Games/
├── Cores/
│   ├── snes9x_libretro.so
│   ├── genesis_plus_gx_libretro.so
│   └── mgba_libretro.so
├── SNES/
│   ├── mario.sfc
│   ├── zelda.sfc
│   └── donkey_country.sfc
├── Genesis/
│   └── sonic.gen
├── GBA/
│   └── pokemon.gba
├── Saves/
└── BIOS/
    ├── psx_scph1001.bin
    └── [other BIOS files]
```

### Run Script Example
```bash
#!/bin/bash
RUSTRETRO="$HOME/Games/rustretro"
CORES="$HOME/Games/Cores"
ROMS="$HOME/Games"
SAVES="$HOME/Games/Saves"
BIOS="$HOME/Games/BIOS"

$RUSTRETRO \
  --core "$CORES/snes9x_libretro.so" \
  --rom "$ROMS/SNES/mario.sfc" \
  --save-dir "$SAVES" \
  --system-dir "$BIOS"
```

## Multi-Core Examples

### SNES with snes9x
```bash
./target/release/rustretro \
  --core ./cores/snes9x_libretro.so \
  --rom ./roms/final_fantasy3.sfc \
  --scale 3
```

### Sega Genesis with Genesis Plus GX
```bash
./target/release/rustretro \
  --core ./cores/genesis_plus_gx_libretro.so \
  --rom ./roms/sonic_the_hedgehog.gen \
  --scale 2
```

### Game Boy Advance with mGBA
```bash
./target/release/rustretro \
  --core ./cores/mgba_libretro.so \
  --rom ./roms/pokemon_ruby.gba \
  --scale 4 \
  --no-audio
```

### NES with FCEUmm
```bash
./target/release/rustretro \
  --core ./cores/fceumm_libretro.so \
  --rom ./roms/super_mario_bros.nes \
  --scale 3
```

### Nintendo 64 with Mupen64Plus
```bash
./target/release/rustretro \
  --core ./cores/mupen64plus_libretro.so \
  --rom ./roms/super_mario_64.z64 \
  --scale 2 \
  --system-dir ./bios/n64
```

## Advanced Examples

### High-Performance Setup
```bash
# Release build with all optimizations
cargo build --release

# Run with minimal scaling for performance
./target/release/rustretro \
  --core ./cores/snes9x_libretro.so \
  --rom ./roms/game.sfc \
  --scale 1 \
  --no-audio
```

### Dedicated Directories
```bash
mkdir -p ~/.local/share/rustretro/{cores,saves,bios}
mkdir -p ~/Games/{SNES,Genesis,GBA}

# Copy cores and ROMs, then run:
./target/release/rustretro \
  --core ~/.local/share/rustretro/cores/snes9x_libretro.so \
  --rom ~/Games/SNES/game.sfc \
  --save-dir ~/.local/share/rustretro/saves \
  --system-dir ~/.local/share/rustretro/bios
```

### Testing Multiple Cores
```bash
# Test with different cores
for core in snes9x genesis_plus_gx mgba; do
  echo "Testing $core..."
  ./target/release/rustretro \
    --core ./cores/${core}_libretro.so \
    --rom ./roms/test_rom.* || echo "$core failed"
done
```

### Batch Rom Loading
```bash
#!/bin/bash
# Simple launcher to try different games

CORE_DIR="./cores"
ROM_DIR="./roms"
SAVE_DIR="./saves"
BIOS_DIR="./bios"

RUSTRETRO="./target/release/rustretro"

launch_game() {
  local core=$1
  local rom=$2
  
  $RUSTRETRO \
    --core "$CORE_DIR/${core}_libretro.so" \
    --rom "$ROM_DIR/$rom" \
    --save-dir "$SAVE_DIR" \
    --system-dir "$BIOS_DIR" \
    --scale 2
}

# Launch games
launch_game "snes9x" "mario.sfc"
launch_game "genesis_plus_gx" "sonic.gen"
launch_game "mgba" "pokemon.gba"
```

## Keyboard Controls Quick Reference

| Key | Function |
|-----|----------|
| Arrow Keys | D-Pad movement |
| Z | B button |
| X | A button |
| A | Y button |
| S | X button |
| Enter | Start button |
| Shift | Select button |
| Q | L button |
| W | R button |
| ESC | Quit game |

## Environment Variables (if implemented)

These can be useful for shell scripts:

```bash
export RUSTRETRO_CORE_DIR="./cores"
export RUSTRETRO_ROM_DIR="./roms"
export RUSTRETRO_SAVE_DIR="./saves"
export RUSTRETRO_BIOS_DIR="./bios"

# Example script using env vars
./target/release/rustretro \
  --core "$RUSTRETRO_CORE_DIR/snes9x_libretro.so" \
  --rom "$RUSTRETRO_ROM_DIR/game.sfc" \
  --save-dir "$RUSTRETRO_SAVE_DIR" \
  --system-dir "$RUSTRETRO_BIOS_DIR"
```

## Development Examples

### Building with Debugging Info
```bash
cargo build
# Creates unoptimized binary with debug symbols at target/debug/rustretro
```

### Running Tests
```bash
cargo test
```

### Checking Code Quality
```bash
cargo clippy
cargo fmt --check
```

### Building Documentation
```bash
cargo doc --open
```

## Troubleshooting Examples

### If Core Not Found
```bash
# Wrong:
./target/release/rustretro --core snes9x_libretro.so --rom game.sfc

# Right - use absolute path:
./target/release/rustretro --core ./cores/snes9x_libretro.so --rom ./roms/game.sfc

# Or:
./target/release/rustretro --core /absolute/path/snes9x_libretro.so --rom /absolute/path/game.sfc
```

### If ROM Not Found
```bash
# Check file exists
ls -la ./roms/game.sfc

# Use correct path
./target/release/rustretro --core ./cores/core.so --rom ./roms/game.sfc
```

### Enable Verbose Output
```bash
# Run from within project for detailed error messages
cargo run --release -- --core ./cores/core.so --rom ./roms/game.sfc
```

## Performance Testing

### Measure Build Time
```bash
time cargo build --release
```

### Check Binary Size
```bash
ls -lh target/release/rustretro
```

### Test with Different Scales
```bash
# 1x - native resolution (fastest)
./target/release/rustretro --core ./cores/core.so --rom ./roms/game.rom --scale 1

# 2x - scaled (good balance)
./target/release/rustretro --core ./cores/core.so --rom ./roms/game.rom --scale 2

# 3x - 3x scaling (default)
./target/release/rustretro --core ./cores/core.so --rom ./roms/game.rom --scale 3

# 4x - large window (slowest)
./target/release/rustretro --core ./cores/core.so --rom ./roms/game.rom --scale 4
```

## Real-World Usage Patterns

### Session Management
```bash
# Keep track of what you were playing
echo "Last played: $(date)" >> last_session.log
./target/release/rustretro \
  --core ./cores/snes9x_libretro.so \
  --rom ./roms/game.sfc
```

### Automated Backup
```bash
# Backup saves after playing
./target/release/rustretro \
  --core ./cores/snes9x_libretro.so \
  --rom ./roms/game.sfc
cp -r ./saves ./saves.backup-$(date +%Y%m%d)
```

### Platform-Specific Aliases
```bash
# Add to ~/.bashrc or ~/.zshrc

# Function to run RustRetro with shortcuts
rr() {
  ~/path/to/rustretro/target/release/rustretro \
    --core ~/path/to/cores/${1}_libretro.so \
    --rom ~/path/to/roms/$2 \
    --save-dir ~/path/to/saves \
    --system-dir ~/path/to/bios
}

# Usage:
# rr snes9x mario.sfc
# rr genesis_plus_gx sonic.gen
```
