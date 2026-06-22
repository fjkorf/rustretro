# rustretro

A libretro frontend written from scratch in Rust — Bevy for video, cpal for audio, and a
built-in egui debugger for taking old games apart while they run.

## Why

Emulator frontends are great at *playing* games. I wanted one that's also an **instrument**:
pause any frame, read the 68000's registers, disassemble around the program counter, label the
code regions you've figured out, and bookmark machine states with thumbnails — the kind of
tooling you want when you're reverse-engineering a CPS2 fighter, not just playing it. Rather
than bolt that onto someone else's frontend, I wrote my own: ~4,000 lines of Rust that
dynamically load any libretro core and wire its C callbacks into a Bevy app.

## What it does

- **Loads any libretro core** at runtime via `libloading` — no recompilation to switch systems.
  Tested with Nestopia (NES), Genesis Plus GX (Mega Drive), and MAME 2003-Plus (arcade/CPS2).
- **Renders via Bevy** — all three libretro pixel formats (0RGB1555, XRGB8888, RGB565)
  converted and uploaded as a sprite texture; live FPS/resolution in the title bar.
- **Audio via cpal** — the core's sample callback feeds a ring buffer into a cpal stream.
- **A 10-panel debug overlay** (`--debug`): frame inspector with pixel picker, hex dump,
  VRAM tile viewer, input monitor, M68K/Z80 CPU state with per-frame register deltas, live
  Capstone disassembly (breakpoints, step, run-to-address), audio controls, event log, and
  frame/pixel pause triggers.
- **Regions & bookmarks** — label disassembled code ranges, snapshot machine states with
  64×48 thumbnails, watch a PC heatmap discover the code for you; everything persists to a
  `<rom>.regions.json` sidecar next to your saves.

## How it works

Rust 2021, built on [Bevy](https://bevyengine.org/) (window/rendering) with `bevy_egui` for the
debug UI, [cpal](https://github.com/RustAudio/cpal) for audio, `libloading` for the core FFI,
and [Capstone](https://www.capstone-engine.org/) for M68K disassembly (~0.5 ms/frame). A few
choices shape everything:

- **The emulator runs on Bevy's main thread as a `NonSend` resource** — libretro cores expect
  synchronous, single-threaded execution, and windowing/audio setup must be on the main thread.
- **A static `AtomicPtr` bridges Rust ↔ C callbacks** — cores call C function pointers, which
  can't carry closure state, so free `extern "C"` trampolines recover their instance from a
  pointer set once at startup. It's race-free because `retro_run()` is synchronous.
- **Live state lives behind `Arc<Mutex<DebugState>>`** — the emulation systems write it; the
  egui overlay reads it. Audio is the one truly concurrent piece, on its own cpal thread.

Full deep-dive with diagrams in [ARCHITECTURE.md](ARCHITECTURE.md); libretro gotchas in
[DEBUGGING.md](DEBUGGING.md); where it's headed in [ROADMAP.md](ROADMAP.md).

## Build & run

```bash
cargo build --release

# NES
cargo run --release -- --core ./nestopia_libretro.dylib --rom ./game.nes

# Arcade, with the debugger open
cargo run --release -- --core ./mame2003_plus_libretro.dylib --rom ./game.zip --debug
```

| Flag | Default | Description |
|------|---------|-------------|
| `--core <PATH>` | required | libretro core dynamic library (`.dylib`/`.so`/`.dll`) |
| `--rom <PATH>` | required | ROM / content file |
| `--scale <N>` | `3` | integer window scale |
| `--save-dir <PATH>` | `.` | SRAM / sidecar directory |
| `--system-dir <PATH>` | `.` | BIOS directory |
| `--fullscreen` | off | fullscreen |
| `--no-audio` | off | disable audio |
| `--debug` | off | open the debug overlay on startup |

Input: arrows → D-pad, Z/X/A/S → B/A/Y/X, Q/W → L/R, Enter → Start, Shift → Select.

## Honest limits

`RETRO_ENVIRONMENT_GET_VARIABLE` isn't implemented (cores needing options may misbehave), only
controller port 0 is wired, and there are no save states, rewind, or cheats. It's a debugging
instrument first, a daily-driver emulator second.

---

— Frank Korf · [fkorf.com](https://fkorf.com)
</content>
