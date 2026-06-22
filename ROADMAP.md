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

- [ ] **Upgrade the egui/Bevy stack** — move from egui 0.31 / bevy 0.15 / bevy_egui 0.33 to
      egui 0.33 / bevy 0.18 / bevy_egui 0.39 (new `EguiPrimaryContextPass` schedule). Touches
      the sprite/image/render code; do it litui-free and get green first. **Prerequisite for
      the litui integration below.**
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

## litui integration — let a UI library own the chrome

The plan: adopt [litui](https://github.com/fjkorf/litui) (Markdown → egui, compiled) to own
RustRetro's **UI framework** (window frame, navigation, panel layout) and **all the rote,
standard screens**, so hand-written code is reserved for the few panels that are the actual
point of this project. litui's matching showcase/driver view lives in
`../litui/knowledge/rustretro-showcase.md`.

**Boundary principle.** The line is *not* "generic vs. retro-specific" — it's **shape**:

> List / form / display surfaces → litui. Custom-painted, spatial surfaces → bespoke.

A watch table or breakpoint manager is retro-specific *content* in a generic *shape*, so litui
can own it. The frame inspector is generic content in a bespoke *shape*, so it stays
hand-written. The truly bespoke core is the five **spatial** views below.

**Panel / screen disposition:**

| Surface | Owner | Notes |
|---|---|---|
| Window frame, tab/nav, layout | **litui** | `define_markdown_app!` shell; bespoke panels mount as `[custom]` slot pages |
| Audio controls | **litui** | `[checkbox]` + `[slider]` + `[display]`, near 1:1 |
| Event log | **litui** | maps to litui's `[log]` widget (`Vec<String>`) |
| CPU state readout | **litui** | `foreach`/`[display]` table; delta color via `::$field` |
| Help / about | **litui** | static markdown — first, free win |
| Controls / keybinds | **litui** + hook | list in litui; key-capture is a small Rust callback |
| File access (core/ROM/dirs) | **litui** + dialog | displays/recents in litui; "Browse" calls `rfd` |
| Watch variables | **litui** + live sync | table in litui; values pushed from memory each frame |
| Breakpoint manager | **litui** | list/enable/delete in litui; set-from-disasm stays bespoke |
| Timing / perf | **litui** (mostly) | numbers/log in litui; live graph needs `egui_plot` or a slot |
| Frame inspector, tile viewer, hex dump, disassembly, heatmap | **bespoke** | custom-painted spatial views — the crown jewels; keep them |

**State model.** RustRetro's truth stays `Arc<Mutex<DebugState>>`; litui's generated `AppState`
is a **pure projection**. One `sync(debug, app)` per frame: copy display values down
(`populate_data` pattern), read widget outputs back up. This already matches how RustRetro
works — `create_bookmark` / `save_regions` are "UI flips a bool, the run loop consumes it,"
which *is* litui's event model. Keep `AppState` a dumb viewmodel; never let domain logic into it.

**Sequencing.**

1. Upgrade the egui/Bevy stack (see Near-term) — litui-free, get green.
2. (litui side) Build the `[custom]` escape hatch — needed both for custom widgets *inside* a
   page and for whole bespoke panels to live *as pages* in litui's nav. Critical-path unknown;
   prototype before committing the migration.
3. Move the shell to litui; bespoke panels become `[custom]` slot pages.
4. Port the 3 easy panels (audio, log, CPU) with the sync layer.
5. Build the new standard screens (help → controls → file access) as pure litui.
6. Watch vars / breakpoint manager / timing once live-data sync is proven.

**Risks.** Adopting litui couples RustRetro to litui's egui cadence (needs a litui min-version
policy); the critical path runs through litui's unbuilt `[custom]` hatch; and the "little Rust"
claim must stay measurable — keep the sync glue small and report the real ratio of
litui-owned vs. bespoke surfaces. Don't litui-ify the spatial inspectors for purity's sake.

## Non-goals

- Becoming a general-purpose, configure-everything emulator frontend (RetroArch exists).
  Features earn their place by making a game easier to *take apart*, not just to play.
