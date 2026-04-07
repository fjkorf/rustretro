# RustRetro MAME FFI Investigation - Detailed Findings

## Status
✅ **ROM works in RetroArch** - Confirmed, runs 300+ frames successfully
❌ **ROM crashes in RustRetro** - Segfaults during `load_game()` call in MAME 2003-Plus

## Changes Made
1. ✅ Added `retro_set_audio_sample_batch` callback (found in MAME binary)
2. ✅ Added explicit handling for all major environment commands that RetroArch sets
3. ✅ Callbacks are registered BEFORE `init()` call (correct order)

## Comparison: RetroArch vs RustRetro

### RetroArch Environment Callbacks (via verbose logging)
- GET_VFS_INTERFACE
- GET_LOG_INTERFACE  
- GET_SYSTEM_DIRECTORY
- GET_SAVE_DIRECTORY
- GET_CORE_OPTIONS_VERSION
- SET_CORE_OPTIONS_V2
- GET_VARIABLE (many option queries)
- SET_AUDIO_BUFFER_STATUS_CALLBACK
- GET_LED_INTERFACE
- SET_CONTROLLER_INFO
- SET_ROTATION
- SET_PIXEL_FORMAT (RGB565, not XRGB8888!)
- SET_MESSAGE
- SET_INPUT_DESCRIPTORS
- SET_GEOMETRY

### RustRetro Callbacks Currently Set
- retro_set_environment ✓
- retro_set_video_refresh ✓
- retro_set_audio_sample ✓
- retro_set_audio_sample_batch ✓
- retro_set_input_poll ✓
- retro_set_input_state ✓

**Missing from RustRetro:**
- VFS interface pointer (may not actually be needed)
- Log interface pointer (may not actually be needed)
- Actual audio buffer status callback setup (just returning true)
- Various core option handling

## Critical Observation

**RetroArch uses RGB565 pixel format, not XRGB8888!**

Our environment callback always returns true for SET_PIXEL_FORMAT without checking which format was requested. MAME might be expecting XRGB8888 but we're not confirming we support it.

## Crash Details

- **Crash location**: Inside MAME's C++ code during `retro_load_game()` function
- **Crash type**: Segmentation fault (signal 11)
- **When**: Immediately when called, before any environment callbacks during load_game
- **Pattern**: Same crash point in all 3 MAME cores (2003-Plus, 2003, current)
- **Crash varies**: MAME current gives Bus Error (signal 10) instead

## Likely Root Causes (in priority order)

### 1. **Structure Layout/Alignment Issue**
RetroGameInfoC structure might have different packing or alignment requirements. The `#[repr(C)]` attribute should handle this, but:
- Pointer sizes might be wrong on this architecture
- Field order might matter
- Unused fields might need padding

### 2. **RetroGameInfoC Data Validation**
MAME might validate game_info fields in a way we're not meeting:
- Null pointer checks failing
- Size validation failing
- Path validation during load_game

### 3. **Missing Required Callbacks or State**
MAME might be checking for specific callbacks or state during load_game:
- Audio buffer status callback actually needs to be called/set up  
- Some other callback set we haven't discovered
- Missing initialization variable

### 4. **Platform/Architecture Specific**
- Apple Silicon (M4) specific issue
- macOS specific libretro implementation
- Bitness issue (32-bit vs 64-bit)

## Next Debugging Steps (in order of effort)

### Quick Fixes to Try (5-10 min each)
1. **Return correct pixel format acknowledgement**
   - Check which format MAME requests, return true only for that format
   
2. **Add actual audio buffer status callback**
   - Implement proper callback handler instead of just returning true
   
3. **Test with a simpler MAME game**
   - Try Pac-Man, Space Invaders, or another simple ROM
   - If it works, problem is Asura Blade specific

### Medium Effort (30 min)
4. **Use lldb debugger**
   ```bash
   lldb ./target/release/rustretro
   (lldb) break set -n "retro_load_game"
   (lldb) run --core ... --rom ...
   (lldb) thread info
   (lldb) bt  # backtrace to see MAME call stack
   ```

5. **Compare RetroArch source code**
   - Check how RetroArch constructs RetroGameInfo before calling load_game()
   - Check exact callback setup order in RetroArch

### Complex (1+ hour)
6. **MAME Core Analysis**
   - Get debug symbols for MAME core
   - Run under debugger with breakpoints in MAME code
   - Inspect struct fields at crash point

## Hypothesis to Test

MAME 2003-Plus might be exiting early in load_game due to ROM validation failure. The crash could be:
1. An assertion failure in MAME's ROM validation
2. A null pointer dereference when validation fails
3. C++ exception being thrown (converted to segfault)

If this is true, a different ROM version (exact match for MAME 2003) would load successfully.

## Recommendation

**Option A (Quickest)**: Test with a completely different, simpler arcade ROM (Pac-Man, Street Fighter, etc.) to determine if the problem is:
- ROM version mismatch (would fail with different ROM too)
- Asura Blade specific (would work with other ROMs)

**Option B (Most Informative)**: Use lldb to get stack trace from crash point. This would immediately show us if problem is in:
- ROM validation code
- Memory layout issue  
- Callback invocation

**Option C (Proven)**: Switch to console emulation (NES/SNES) which is simpler than MAME arcade emulation. RustRetro should work fine with those.
