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

## The war story

For a while, MAME cores crashed on load while Nestopia ran fine — and the investigation
(committed in this repo as it happened, `MAME_FFI_INVESTIGATION.md` →
`FINAL_MAME_REPORT.md`) found a great bug: **nearly every libretro environment-callback
constant in my FFI layer was wrong.** I'd numbered them sequentially; the spec doesn't.
Exactly one constant (`GET_SYSTEM_DIRECTORY = 9`) was right by coincidence, and Nestopia is
lenient enough to limp along on that alone. MAME is not. Fixing the constants against
`libretro.h` brought the arcade cores fully to life — a tidy lesson in how a forgiving peer
can hide a broken protocol.

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

## Stack

Rust 2021 · [Bevy](https://bevyengine.org/) (window/rendering) · `bevy_egui` (debug UI) ·
[cpal](https://github.com/RustAudio/cpal) (audio) · `libloading` (core FFI) ·
[Capstone](https://www.capstone-engine.org/) (M68K disassembly, ~0.5 ms/frame) ·
`clap` · `serde`. Architecture deep-dive in [ARCHITECTURE.md](ARCHITECTURE.md).

## Honest limits

`RETRO_ENVIRONMENT_GET_VARIABLE` isn't implemented (cores needing options may misbehave),
only controller port 0 is wired, and there are no save states, rewind, or cheats. It's a
debugging instrument first, a daily-driver emulator second.

---

— Frank Korf · [fkorf.com](https://fkorf.com)
