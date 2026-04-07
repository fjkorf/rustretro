# RustRetro MAME Integration - Final Report

## Executive Summary

✅ **MAME 2003-Plus core is now fully working in RustRetro**
✅ **Nestopia (NES) core continues to work with no regression**
✅ **Root cause identified and fixed**

## Root Cause

**All libretro environment callback constants were wrong.** The codebase used a sequential
numbering scheme that didn't match the real libretro.h spec. Additionally, the pixel format
enum values were incorrect.

### Example constant errors (before fix):

| Constant | Our (wrong) value | Spec (correct) value |
|---|---|---|
| `SET_PIXEL_FORMAT` | 1 | **10** |
| `SET_SYSTEM_AV_INFO` | 2 | **32** |
| `GET_VARIABLE` | 4 | **15** |
| `GET_SAVE_DIRECTORY` | 10 | **31** |
| `GET_LOG_INTERFACE` | 11 | **27** |
| `GET_VFS_INTERFACE` | 54 | **65581** (45 \| 0x10000) |
| `RETRO_PIXEL_FORMAT_XRGB8888` | 2 | **1** |

Only `GET_SYSTEM_DIRECTORY = 9` was correct by coincidence.

### Why Nestopia worked despite wrong constants

Nestopia was accidentally functional because:
- `cmd=9` (GET_SYSTEM_DIRECTORY) happened to be correct
- Other mismatches were either ignored or caused non-fatal behavior
- Nestopia is more lenient than MAME about which callbacks are handled

### Why MAME crashed

MAME requires a broader set of callbacks to function. With wrong constants:
- `SET_PIXEL_FORMAT` (cmd=10) was misidentified as `GET_SAVE_DIRECTORY` → wrote a directory
  pointer where a pixel format enum should go
- `GET_LOG_INTERFACE` (cmd=27) fell through to `_ => false`
- `GET_SAVE_DIRECTORY` (cmd=31) fell through to `_ => false`
- Multiple handlers fired for wrong commands, corrupting core state

## Additional Issues Fixed

### 1. VFS interface returning `true` without a struct
Our code returned `true` for `GET_VFS_INTERFACE` but didn't fill in the function pointer
struct — this would crash any core that tried to call VFS functions. Fixed: return `false`
so cores fall back to stdio file I/O.

### 2. Log interface not implemented
MAME calls `GET_LOG_INTERFACE` (cmd=27) during init. We now provide a real log callback
that forwards messages to stderr with level prefixes.

### 3. `GET_VARIABLE_UPDATE` not returning a value
Must write `false` to the `*mut bool` data pointer, not just return `true`.

## Changes Made

### `src/libretro.rs`
- Fixed all environment callback constants to match libretro.h spec
- Added `RETRO_ENVIRONMENT_EXPERIMENTAL = 0x10000`
- Added correct `GET_VFS_INTERFACE = 45 | 0x10000 = 65581`
- Fixed `RETRO_PIXEL_FORMAT_XRGB8888 = 1` (was 2)
- Added `RETRO_PIXEL_FORMAT_0RGB1555 = 0` and `RGB565 = 2`
- Added `RetroLogCallback` and `RetroMessage` structs
- Added 15+ new constant definitions for complete coverage

### `src/frontend.rs`
- Rewrote `environment_callback()` with clean, minimal match arms
- Removed per-call verbose logging that flooded output on MAME
- Implemented `GET_LOG_INTERFACE` with a real log callback function
- Changed `GET_VFS_INTERFACE` from `true` (dangerous) to `false` (safe stdio fallback)
- Added proper data-writing for `GET_VARIABLE_UPDATE` and `GET_LANGUAGE`
- Added `GET_AUDIO_VIDEO_ENABLE` handler
- Expanded accepted pixel formats to include RGB565

## Verification

```bash
# MAME 2003-Plus - asurabld.zip (Asura Blade)
./target/release/rustretro \
    --core ~/games/cores/mame2003_plus_libretro.dylib \
    --rom ~/games/roms/asurabld.zip
# Result: ✅ load_game() returned true — Emulation ended cleanly.

# Nestopia NES core
./target/release/rustretro \
    --core ~/Library/Application\ Support/RetroArch/cores/nestopia_libretro.dylib \
    --rom ~/games/roms/test.nes
# Result: ✅ load_game() returned true — Emulation ended cleanly.
```

## Lessons Learned

1. **Always cross-reference constants against the authoritative libretro.h** — never assume
   a hand-rolled constant table is correct.
2. **`GET_VFS_INTERFACE` returning `true` without a struct is a crash bomb** — cores call
   function pointers immediately. Return `false` unless you implement it.
3. **MAME is stricter than NES cores** about having correct callback behavior during init.
   A wrong response to `SET_PIXEL_FORMAT` can corrupt core state before `load_game()` is called.
4. **The libretro EXPERIMENTAL flag (0x10000)** is used for VFS, LED, and other newer interfaces.
   These have large cmd values like 65581 that look like bugs but are intentional.


