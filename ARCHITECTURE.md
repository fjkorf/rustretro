# RustRetro Architecture & Implementation Details

## Overview

RustRetro is a lightweight libretro frontend composed of ~900 lines of Rust code across 4 modules. It uses dynamic library loading to work with any libretro core without recompilation.

## Module Breakdown

### 1. `main.rs` (78 lines)
**Purpose**: CLI interface using `clap` for argument parsing

**Key Components**:
- `Args` struct with `clap` derive macros
- Command-line argument parsing with defaults
- Input validation (core and ROM file existence checks)
- Startup logging

**Arguments Supported**:
```
--core <PATH>          (required) Path to libretro core .so/.dll/.dylib
--rom <PATH>           (required) Path to ROM/content file
--fullscreen           (flag) Enable fullscreen mode
--save-dir <PATH>      (opt) Save file directory, default: .
--system-dir <PATH>    (opt) BIOS directory, default: .
--scale <FACTOR>       (opt) Window scale (1-10), default: 3
--no-audio             (flag) Disable audio output
```

### 2. `libretro.rs` (281 lines)
**Purpose**: FFI bindings to libretro cores and core management

**Key Types**:
- `RetroCore`: Represents a loaded libretro core
  - Methods: `load()`, `get_system_info()`, `set_callbacks()`, `init()`, `load_game()`, `run()`, `unload_game()`, `deinit()`
  
- `RetroSystemInfo`: Core metadata (name, version, extensions, fullpath requirement)
- `RetroGameInfo`: Game file information (path, data)
- `RetroSystemAVInfo`: Video/audio configuration (dimensions, aspect ratio, FPS, sample rate)

**Callback Types**:
- `RetroEnvironmentFn`: Core query callback (pixel format, directories, etc.)
- `RetroVideoRefreshFn`: Framebuffer delivery callback
- `RetroAudioSampleFn`: Audio sample callback  
- `RetroAudioSampleBatchFn`: Batch audio samples callback
- `RetroInputPollFn`: Input polling callback
- `RetroInputStateFn`: Button state query callback

**Constants**:
- Environment callback command IDs
- Pixel format definitions (XRGB8888)
- Input device IDs and button mappings

**Implementation Notes**:
- Uses `libloading` for dynamic library loading
- Wraps C types (`c_char`, `c_void`) for FFI
- Converts C strings to Rust strings safely
- Error handling with `thiserror::Error` enum

### 3. `sdl_interface.rs` (188 lines)
**Purpose**: SDL2 abstraction for graphics, audio, and input

**Key Structures**:

**Graphics**:
- Minimal stub implementation (window creation verified)
- Method: `new()` creates SDL window
- Method: `render_frame()` for texture updates (not fully rendering)
- Method: `set_dimensions()` for resize handling

**Audio**:
- `Audio` struct wraps SDL2 audio device
- Method: `new()` initializes audio context
- Method: `queue_sample()` queues audio samples
- Method: `process_queue()` processes pending samples
- Uses `AudioCallback` trait for SDL2 integration

**Input**:
- `Input` struct maintains button state array [bool; 12]
- Maps SDL2 keycodes to SNES-style buttons:
  - Arrow Keys → D-Pad
  - ZXAS → BYXA buttons
  - Q/W → L/R buttons
  - Enter → Start
  - Shift → Select
- Methods: `handle_event()`, `get_button_state()`

**Tests**: Basic input initialization test included

### 4. `frontend.rs` (321 lines)
**Purpose**: Main frontend logic, callback system, and main loop

**Key Structures**:

**Frontend**:
- Owns: `RetroCore`, `Graphics`, `Audio`, `Input`
- Owns: `CallbackContext` with frame/audio/input data
- Methods:
  - `new()`: Initializes core, loads game, sets up callbacks
  - `setup_callbacks()`: Registers static callback functions
  - `run()`: Main event loop with frame timing

**CallbackContext**:
- Public fields for callback access
- Methods (private):
  - `environment_callback()`: Handles core environment queries
  - `video_callback()`: Stores framebuffer data
  - `audio_callback()`: Queues audio samples
  - `input_poll_callback()`: SDL2 event polling trigger
  - `input_state_callback()`: Returns button states

**Static Callback Functions**:
- `static_environment_callback()`
- `static_video_callback()`
- `static_input_poll_callback()`
- `static_input_state_callback()`
- `static_audio_callback()`

These functions use an atomic pointer to access the callback context without capturing variables.

**Global State**:
```rust
static CALLBACK_CONTEXT: AtomicPtr<CallbackContext> = AtomicPtr::new(std::ptr::null_mut());
```

This allows C callbacks to access Rust state through a thin FFI layer.

## Data Flow

```
User starts program
        ↓
[main.rs] Parse CLI arguments
        ↓
[frontend.rs] Create Frontend
        ├→ [libretro.rs] Load core .so file
        ├→ [sdl_interface.rs] Create SDL window
        └→ Set up callbacks and initialize core
        ↓
[main.rs] Call frontend.run()
        ↓
[frontend.rs] Main loop (SDL event pump)
   ├→ Poll SDL events (keyboard input)
   ├→ Store button states in CallbackContext
   ├→ Call core.run()
   │   └→ Core executes C functions
   │       ├→ Video callback: data → framebuffer
   │       ├→ Audio callback: samples → queue
   │       └→ Input callback: query button states
   ├→ Process audio queue
   ├→ Limit frame rate
   └→ Repeat or exit on Quit event
        ↓
Clean up: unload_game(), deinit(), close SDL
```

## Callback Mechanism

### Why Static Callbacks?

Libretro cores are C code that call function pointers. These pointers must be C function pointers (`extern "C"`), not Rust closures. Closures can't be function pointers if they capture state.

### Solution: Atomic Pointer Pattern

```rust
static CALLBACK_CONTEXT: AtomicPtr<CallbackContext> = ...;

extern "C" fn static_video_callback(data: *const c_void, ...) {
    unsafe {
        let ctx_ptr = CALLBACK_CONTEXT.load(Ordering::SeqCst);
        if !ctx_ptr.is_null() {
            (*ctx_ptr).video_callback(data, ...);
        }
    }
}
```

1. Store context pointer in static atomic
2. In static callback, load pointer with atomic operation
3. Cast pointer back to Rust struct and call methods
4. Thread-safe due to atomic operations

### Environment Callbacks

Handles core queries:
- `SET_PIXEL_FORMAT`: Confirms XRGB8888 format
- `GET_SYSTEM_DIRECTORY`: Returns path for BIOS files
- `GET_SAVE_DIRECTORY`: Returns path for save files
- `SET_SYSTEM_AV_INFO`: Core provides FPS/sample rate
- `GET_VARIABLE`: Core options (not implemented)

## Dependencies & Versions

```toml
clap = "4.5"          # CLI argument parsing
sdl2 = "0.36"         # Graphics/audio/input
libloading = "0.8"    # Dynamic library loading
thiserror = "1.0"     # Custom error types
anyhow = "1.0"        # Error handling
parking_lot = "0.12"  # Efficient synchronization
```

## Error Handling

**Custom Error Type** (`LibretroError`):
- `LoadFailed(String)` - Core loading failure
- `ApiVersionMismatch` - Incompatible core version
- `CoreNotLoaded` - Function called before load
- `GameLoadFailed` - ROM couldn't be loaded

**Fallback**: Uses `anyhow::Result` for other errors (file I/O, SDL2)

## Known Limitations

### Not Implemented Yet
- ❌ **Video Rendering**: Framebuffer captured but not rendered to SDL texture
- ❌ **Audio Playback**: Callback registered but samples not played
- ❌ **Save States**: Infrastructure present but file I/O not done
- ❌ **Core Options**: Env callback returns false for variable queries
- ❌ **Multi-controller**: Only supports port 0
- ❌ **Cheats/Rewind**: No support yet

### Design Limitations
- Single-threaded (core execution blocks main loop)
- No frame skip or performance throttling
- No screen capture or recording
- No disc/multi-file content

## Performance Characteristics

- **Binary Size**: ~800KB release build
- **Memory Usage**: Minimal (core-dependent)
- **CPU Usage**: Depends on core/game, frame rate limiting included
- **Latency**: Minimal (directly calls core each frame)

## Testing

Current test coverage:
- `Input` button state initialization

Future tests needed:
- Core loading and unloading
- Callback system integration
- SDL2 event handling
- Frame rate limiting accuracy

## Extension Points

To enhance the frontend:

1. **Video**: Implement SDL2 texture rendering in `Graphics::render_frame()`
2. **Audio**: Integrate SDL audio queue with callback samples
3. **Options**: Implement `RETRO_ENVIRONMENT_GET_VARIABLE` for core options
4. **Save States**: Add serialization to disk
5. **Rewind**: Buffer frames in ring buffer
6. **Controllers**: Support multiple ports with device selection

## Code Style

- Error handling via `Result` types
- Minimal unsafe (only in FFI boundaries)
- Clear separation of concerns (modules)
- Documentation on public APIs
- Direct style (no unnecessary abstractions)

## Compile-Time Guarantees

- Type-safe FFI with `extern "C"`
- Memory-safe callback system (via static dispatch)
- No unsafe iterator or unwrap() in hot paths
- Safe string conversions from C
