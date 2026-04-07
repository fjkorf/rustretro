# RustRetro - Lightweight Libretro Frontend

A minimal, functional libretro frontend written in Rust with a CLI interface. This frontend can load and run libretro cores with proper FFI integration, SDL2 for graphics/audio/input, and clean error handling.

## Features

- **Dynamic core loading** using `libloading` - no compile-time dependencies on specific cores
- **CLI argument parsing** with `clap` for intuitive command-line interface
- **SDL2 integration** for window management, rendering, and input handling
- **Libretro callback system** with environment, video, audio, and input callbacks
- **Frame rate limiting** based on core AV info
- **Keyboard input mapping**:
  - Arrow keys → D-pad (Up/Down/Left/Right)
  - Z → B button
  - X → A button
  - A → Y button
  - S → X button
  - Enter → Start
  - Shift → Select
  - Q → L button
  - W → R button

## Prerequisites

- Rust 1.70+ (or 2021 edition)
- SDL2 development libraries
  - macOS: `brew install sdl2`
  - Ubuntu/Debian: `sudo apt-get install libsdl2-dev`
  - Fedora: `sudo dnf install SDL2-devel`
- A libretro core (.so/.dll/.dylib file)
- A compatible ROM/content file

## Building

```bash
cargo build --release
```

The compiled binary will be at `target/release/rustretro`.

## Usage

```bash
cargo run -- \
  --core /path/to/core.so \
  --rom /path/to/game.rom \
  --scale 3 \
  --save-dir ./saves \
  --system-dir ./bios
```

### Command-line Arguments

- `--core <PATH>` (required): Path to the libretro core dynamic library
- `--rom <PATH>` (required): Path to the game ROM or content file
- `--scale <FACTOR>` (default: 3): Window scale factor (1 = native, 2 = 2x, etc.)
- `--save-dir <PATH>` (default: .): Directory for save files and save states
- `--system-dir <PATH>` (default: .): Directory for BIOS/system files
- `--fullscreen`: Start in fullscreen mode
- `--no-audio`: Disable audio output

### Examples

```bash
# Run SNES game with default settings
cargo run -- --core ./snes9x_libretro.so --rom ./mario.sfc

# Run Genesis game fullscreen at 4x scale
cargo run -- \
  --core ./genesis_plus_gx_libretro.so \
  --rom ./sonic.gen \
  --scale 4 \
  --fullscreen

# Run with custom save/system directories
cargo run -- \
  --core ./mgba_libretro.so \
  --rom ./pokemon.gba \
  --save-dir ~/.local/share/rustretro/saves \
  --system-dir ~/.local/share/rustretro/bios
```

## Architecture

### Modules

- **`main.rs`**: CLI entry point using `clap`
- **`libretro.rs`**: Libretro FFI bindings and core loading
- **`sdl_interface.rs`**: SDL2 wrapper for graphics, audio, and input
- **`frontend.rs`**: Main frontend logic, callback system, and main loop

### Callback System

The frontend implements all essential libretro callbacks:

- **Environment Callback**: Handles core queries for system info, save directories, pixel format, etc.
- **Video Refresh**: Receives framebuffer data for rendering
- **Audio Callback**: Receives audio samples (basic implementation)
- **Input Poll**: Polled when core needs input state
- **Input State**: Returns button press states for joypad input

Static callback functions are used with atomic pointer storage to allow libretro's C interface to call back into Rust.

## Implementation Details

### Libretro Core Loading

The frontend uses `libloading` to dynamically load libretro cores at runtime. This allows running any compatible core without recompilation.

Key libretro functions loaded:
- `retro_api_version()` - Verify API compatibility
- `retro_get_system_info()` - Get core metadata
- `retro_set_*()` - Register callbacks
- `retro_init()` - Initialize core
- `retro_load_game()` - Load ROM
- `retro_run()` - Execute one frame
- `retro_unload_game()` - Clean up
- `retro_deinit()` - Shutdown

### Callback Context

A `CallbackContext` struct maintains state needed by callbacks:
- Save and system directories
- Current framebuffer data
- Input button states
- Video dimensions

The context is stored as a static `AtomicPtr` to allow C callbacks access without Rust closure captures.

### Frame Timing

Frame rate is limited based on the AV info provided by the core. The main loop sleeps appropriately to maintain target FPS.

## Limitations & Future Improvements

### Current Limitations

- **Audio**: Basic structure but not fully integrated with SDL2 playback
- **Video**: Framebuffer stored but not rendered to window
- **Core Options**: Environment callback doesn't handle core options (`RETRO_ENVIRONMENT_GET_VARIABLE`)
- **Multiple Controllers**: Only supports single joypad (port 0)
- **Save States**: Directory provided but not implemented

### Potential Enhancements

1. Complete SDL2 renderer integration for proper frame display
2. Full audio sample buffering and playback
3. Save state support (with serialization)
4. Multi-controller support
5. Core options dialog
6. Runtime configuration via config files
7. Disc/multi-file content support
8. Cheats/cheat codes
9. Rewind functionality
10. Screenshot/recording

## Error Handling

The frontend uses `anyhow::Result` for flexible error handling and `thiserror::Error` for custom error types. All major operations report detailed errors:

- File I/O failures
- Core loading failures
- Game loading failures
- SDL2 initialization failures
- API version mismatches

## Dependencies

- `clap`: Command-line argument parsing
- `sdl2`: Graphics/audio/input (0.36)
- `libloading`: Dynamic library loading
- `thiserror`: Custom error types
- `anyhow`: Error handling
- `parking_lot`: Synchronization primitives

## Testing

Run the test suite:

```bash
cargo test
```

Currently includes basic input state tests. More comprehensive tests would require actual libretro cores.

## License

This is a reference implementation for educational and personal use.

## References

- [Libretro API Documentation](https://github.com/libretro/libretro.github.io)
- [SDL2 Rust Bindings](https://docs.rs/sdl2/)
- [Libretro Cores](https://www.libretro.com/index.php/api/)
