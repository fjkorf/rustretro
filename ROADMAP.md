# RustRetro — Roadmap

RustRetro is a **debugging instrument first, a daily-driver emulator second.** The roadmap
reflects that: depth on the reverse-engineering tooling, breadth on emulator features only
where it serves that goal. Status reflects the codebase as of the current branch.

## Done

- Dynamic libretro core loading (`libloading`), correct env-callback constants
- Bevy rendering for all three pixel formats; cpal audio
- 10-panel egui debugger: frame inspector, hex, tiles, input, CPU state, disassembly,
  audio, log, triggers, regions
- M68K disassembly via Capstone, sourced from `SekFetchByte` / `SET_MEMORY_MAPS`
- Breakpoints, single-step, run-to-address, per-frame register deltas
- PC heatmap, code-region labeling (inline from the Disasm panel), state bookmarks with
  thumbnails, persisted to a `<rom>.regions.json` sidecar

## Near-term (next)

- [ ] **`RETRO_ENVIRONMENT_GET_VARIABLE`** — real core-options support. Today it returns
      false, so cores needing options can misbehave. Highest-leverage correctness fix.
- [ ] **Reconcile + verify MAME path** — confirm `retro_load_game` success across a couple of
      MAME 2003-Plus ROMs and document the BIOS/`--system-dir` requirements.
- [ ] **Dead-code sweep** — ~28 dead-code warnings (unused FFI constants, methods, fields).
      Keep the ones that document the protocol; cut the rest.
- [ ] **Move `--test-capstone` / `--test-phase2`** out of `main.rs` into real `#[test]`s
      (currently `src/capstone_test.rs`, `src/phase2_test.rs` are scaffolds behind hidden flags).

## Mid-term

- [ ] **Z80 disassembly** — CPU panel already reads Z80 PC; extend the Disasm panel to Z80
      (second core in Genesis/arcade hardware).
- [ ] **Multi-port input** — only joypad port 0 is wired; add port 1+ for 2-player titles.
- [ ] **Memory watch / search** — find values, set watchpoints, track changes across frames
      (a natural companion to the hex dump and heatmap).
- [ ] **Save states** — wire `retro_serialize` / `retro_unserialize`; foundation for rewind.

## Later / exploratory

- [ ] **Rewind** (depends on save states)
- [ ] **Cheat / patch support** (`retro_cheat_set`)
- [ ] **Symbol import/export** — load labels from a `.sym`/IDA/Ghidra map into code regions
- [ ] **Trace logging** — record PC/register history to disk for offline analysis
- [ ] **Disc / multi-file content** support

## Non-goals

- Becoming a general-purpose, configure-everything emulator frontend (RetroArch exists).
  Features earn their place by making a game easier to *take apart*, not just to play.
