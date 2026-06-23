# RustRetro — Roadmap

RustRetro is a **debugging instrument first, a daily-driver emulator second.** The roadmap
reflects that: depth on the reverse-engineering tooling, breadth on emulator features only
where it serves that goal. Status reflects the codebase as of the current branch.

## Done

**Foundation**
- Dynamic libretro core loading (`libloading`), correct env-callback constants
- Bevy rendering for all three pixel formats; cpal audio (volume/mute via shared atomics,
  applied at drain time)
- M68K disassembly via Capstone, sourced from `SekFetchByte` / `SET_MEMORY_MAPS`
- Breakpoints, single-step, run-to-address, per-frame register deltas
- PC heatmap, code-region labeling (inline from the Disasm panel), state bookmarks with
  thumbnails, persisted to a `<rom>.regions.json` sidecar

**Reverse-engineering tooling (waves 1–6)**
- **Watch panel** — pinned named addresses, live values, freeze/lock write-back, multiple
  display formats
- **RAM Search** — cheat-engine iterative narrowing (=, ≠, <, >, changed/unchanged/
  increased/decreased/different-by; vs previous snapshot or specific value), "+Watch" handoff
- **"What changed this address?"** — per-watch frame-granular change log (frame · old→new · PC)
- **Hex dump** with changed-cell amber tint
- **VDP register panel** — decodes Genesis VDP registers $00–$17 to plain-English bitfields
  (decode-ready; live source not exposed by the cores — see `src/debug/vdp_source.rs`)
- **Lua scripting** (`mlua`, sandboxed) — `memory.*`, `gui.draw*`, `event.onframeend`,
  `console.log`, `emu.framecount`, drawn into the framebuffer pre-blit; in-UI script panel (F10)
- **egui_dock workspace** — draggable/splittable multi-panel layout (14 surfaces visible at
  once), persisted to `rustretro_layout.json`
- **Persistent toolbar + linked navigation** — Back/Fwd history, Run/Pause/Step, Go-to-address,
  PC readout; a shared address cursor (`goto`) drives Disasm + Hex from Regions/Watch/Search

## Near-term (next)

- [x] ~~**Upgrade the egui/Bevy stack**~~ — SHIPPED: bevy 0.18.1 / bevy_egui 0.39.1 /
      egui 0.33.3 / egui_dock 0.18 (single egui in the tree). Unblocked the litui integration.
- [x] ~~**`retro_get_memory_data` fallback**~~ — SHIPPED & proven live (GPGX + Mortal Kombat II:
      0 regions → 64 KB System RAM, 1169 nonzero bytes). Unlocks Genesis + CPS2 work-RAM. Full
      VRAM/sprite/ROM still needs `SET_MEMORY_MAPS` or a hardcoded region table.
- [ ] **`RETRO_ENVIRONMENT_GET_VARIABLE`** — real core-options support. Today it returns
      false, so cores needing options can misbehave. Highest-leverage correctness fix.
- [ ] **Mesen NES core fails to load** — `genesis_plus_gx`/`fceumm` load fine, but the Mesen
      libretro core exits silently during `retro_load_game` in RustRetro (no panic, no AV info).
      Investigate (likely a `need_fullpath` / system-dir / env-callback expectation). fceumm is the
      working NES core meanwhile.
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
      Critical for fighting games (P2 inputs).
- [x] ~~**Memory watch / search**~~ — shipped (Watch panel, RAM Search, change tracking).
- [ ] **Save states** — wire `retro_serialize` / `retro_unserialize`; foundation for rewind,
      and the backing for Lua `savestate.*` (stubbed out of the v1 API today).

## Later / exploratory

- [ ] **Hitbox / hurtbox overlay** — read object-RAM box lists → translucent rects on the
      framebuffer (the fighting-game differentiator). The Lua `gui.draw*` layer already supports
      this for community scripts; a built-in per-game overlay is the next step.
- [ ] **Frame meter** — per-frame phase strip (startup/active/recovery), one row per player,
      for reading frame advantage at a glance.
- [ ] **Instruction-level "what writes this address"** — today's change tracking is
      frame-granular (no libretro per-access hook); true instruction-exact needs a core debug
      interface or a trace-correlation pass.
- [ ] **VDP register live source** — intercept M68K control-port writes to `$C00004/$C00006`
      to populate the (already-built) VDP decoder panel.
- [ ] **Rewind** (depends on save states)
- [ ] **Cheat / patch support** (`retro_cheat_set`) — the Watch freeze path is a partial start
- [ ] **Symbol import/export** — load labels from a `.sym`/IDA/Ghidra map into code regions
- [ ] **Trace logging** — record PC/register history to disk for offline analysis
- [ ] **Disc / multi-file content** support

## litui integration — let a UI library own the chrome

The plan: adopt [litui](https://github.com/fjkorf/litui) (Markdown → egui, compiled) to own
RustRetro's **UI framework** (window frame, navigation, panel layout) and **all the rote,
standard screens**, so hand-written code is reserved for the few panels that are the actual
point of this project. litui's matching showcase/driver view lives in
`../litui/knowledge/rustretro-showcase.md`.

> **Repo ownership:** the litui library (`../litui`) is developed in its own dedicated
> session/repo. RustRetro-side work treats litui as an **external dependency it consumes**, never
> modifies — the `[custom]` escape-hatch GO (Wave B) and the parser-crate refactor land on the
> litui side. RustRetro's litui Waves C–F begin once litui ships a version with `[custom]`; pin
> that version then. Don't edit `../litui` from a RustRetro session.

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

### Sequenced execution plan — "tutorials working in litui"

The target milestone for the integration phase: **the 15 tutorial pages already authored in
`docs/tutorials/` render as in-app litui screens (Help → Tutorials).** They are pure
display/document content with no state round-trip, so they are the *lowest-risk first real
litui surface* — a better first mount than a form. Everything below is bundled to reach that
milestone, then the spatial panels follow.

Each wave is built **in a dedicated git worktree** (isolated from `master`/the PR branch) by
sized agents, integrated to a green build, then committed. Build stays green between waves.

| Wave | Goal | Agents (size) |
|---|---|---|
| **A — stack upgrade** | egui 0.31→0.33, bevy 0.15→0.18, bevy_egui 0.33→0.39 (`EguiPrimaryContextPass`); bump `egui_dock` to the egui-0.33 release in lockstep. litui-free; get green first. | 1 opus (render/schedule churn) + 1 sonnet (egui_dock/API-rename sweep) |
| **B — `[custom]` escape-hatch prototype** | The critical-path unknown: prove `[custom](slot)` invoking an `FnMut(&mut egui::Ui)` on a macro-generated `AppState` works across the proc-macro boundary, in the **Bevy** path. Spike one bespoke panel (Hex) as a `[custom]` page. **Go/no-go gate** for the whole migration. | 1 opus (spike) |
| **C — litui shell + live-resource binding** | Mount `define_markdown_app!` as the window frame/nav; prove per-frame `populate_data` (DebugState → `[display]`) in Bevy. Port the 3 easy panels (Audio, Log, CPU) as litui pages with the sync layer. | 1 opus (shell + sync) + 1 sonnet (3 panel ports) |
| **D — tutorials as litui pages (the milestone)** | Wire `docs/tutorials/*.md` (+ `_tutorials.md` parent) into the app as a Help → Tutorials nav group via `define_markdown_app!`. Static render first; this is the milestone commit. | 1 sonnet (mounting + nav) |
| **E — bespoke panels as `[custom]` pages** | Move the spatial inspectors (Frame, Tiles, Hex, Disasm, heatmap) into litui's nav as `[custom]` slot pages; the dock workspace either wraps or is superseded by litui nav (decide during B/C). | 1 opus + 1 sonnet |
| **F — live tutorial embeds** | Upgrade the tutorial pages' annotated `<!-- litui:live -->` points to real `[display]`/`[custom]` embeds (live watch values, RAM Search candidate count, script output) — tutorials that *run inside the tool*. | 1 sonnet per tutorial cluster |

**Then: the tutorial / workflow-hardening phase** (separate from the above, begins after the
user's review of the current static tutorials). Walk each tutorial against the live app, fix the
friction it exposes, and harden navigation/workflow paths. Known seeds for this phase (surfaced
while authoring the tutorials):

- **Pause/Step is triplicated** across the toolbar, Disasm, and Triggers panels; the toolbar's
  "Step Frame" currently just single-steps (no real run-to-next-frame). Unify the controls.
- **No "send to trigger" link** from the Frame Inspector's pixel picker to the Triggers fields —
  the user re-types coordinates by hand.
- **VDP ↔ Tiles are conceptually linked but disconnected** (VDP regs name VRAM bases the Tile
  viewer could show); also VDP stays un-wired until the control-port source lands.

**Risks.** The migration's critical path runs through litui's **unbuilt `[custom]` hatch**
(Wave B is the gate — if the `FnMut`-on-`AppState` lifetime is intractable, the "litui owns the
frame" claim fails and we keep the egui_dock shell). Adopting litui couples RustRetro to litui's
egui cadence (needs a min-version policy); `egui_dock` must move in lockstep with the egui bump.
Keep the sync glue small and report the real ratio of litui-owned vs. bespoke surfaces; don't
litui-ify the spatial inspectors for purity's sake.

## AI-friendly interface — converse with Claude about the running app

The goal: drive a RustRetro session from a Claude conversation — *"identify the ROM that holds
the sprite pieces for the characters currently on screen,"* *"freeze player 2's health and label
the routine that drains it"* — with Claude perceiving the live app and acting on it. This is a
parallel track to the litui work, not a competitor: both read the same `Arc<Mutex<DebugState>>`
hub. litui is the **human** UI surface; this is the **AI** surface.

**Why we're already ~70% there.** `DebugState` already centralizes everything an AI would read
(memory regions, M68K/Z80 registers, `fb_rgba`, `pc_heatmap`, `watches`, `change_log`,
`code_regions`, `bookmarks`, `frame_count`), much of it already `serde`-serializable.
`DebugState::read_addr`/`read_u8` already abstract guest-memory reads. The Lua engine is already
a programmatic control surface. The `.regions.json` sidecar and `ROM_MAP_FORMAT.md` maps are
already designed for tool+human co-authoring — AI becomes the third co-author.

**The architecture: an MCP server over `DebugState`.** MCP is the protocol a Claude session
already uses to talk to tools, so RustRetro exposes one. Claude connects and gets:
- *Resources (perception):* `app://state` (serialized DebugState), `app://screen` (framebuffer
  as PNG — gives Claude **vision** of the game for free), `app://memory/{region}/{addr}/{len}`,
  `app://watches`, `app://regions`, `app://heatmap`, `app://change-log`, `app://rom-map`.
- *Tools (action):* `pause`/`step`/`run_to`, `read_mem`/`write_mem`, `add_watch`/`freeze`,
  `ram_search`, `set_breakpoint`, `bookmark`, `label_region`, and `run_lua(script)` — the
  escape hatch that makes the Lua engine **Claude's hands** for exploratory probes.

**The honest gap: perception, not action.** Acting (pause/step/read/write/watch/search) is
nearly free — it's already `DebugState` methods. *Perceiving* is the new work, and the sprite
query demands it in three steps: (1) what's on screen → **sprite/OAM decode** (no object-RAM
model today); (2) which VRAM tiles those sprites use → sprite→tile mapping; (3) where those
tiles came from in ROM → **VRAM→ROM provenance** (DMA source→dest logging — the genuinely new
core capability, and it shares the VDP control-port-intercept work already roadmapped above).

**Sequenced sub-track** (each a wave; sequencing against the litui waves TBD):

| # | Capability | Effort | Notes |
|---|---|---|---|
| 1 | **MCP server over `DebugState`** — resources (state/screen/memory) + action tools | M | The whole interface; a working "talk to Claude about the app" loop. Screen-as-PNG = vision. |
| 2 | **`run_lua` MCP tool** | S | Reuses the engine; Claude writes/runs probes. Huge leverage, tiny cost. |
| 3 | **Sprite / OAM decode** — model active sprites + their VRAM tile refs | M | "What's on screen" structurally; also powers the roadmapped hitbox overlay. |
| 4 | ~~**VRAM→ROM provenance** — DMA source→dest logging~~ | M–L | **Spiked 2026-06: INFEASIBLE on stock cores.** libretro exposes no per-write hook and neither genesis_plus_gx nor fbalpha2012 export a write-intercept/VDP-peek symbol; true DMA-event tracing needs a patched core. Stock-core substitute = convergent CONTENT/STRUCTURE evidence (`vram_to_rom` + `render_tiles` + `scan_regions`), which is honestly *not* DMA-traced. See `src/debug/vdp_source.rs`. |
| 5 | **Structured AI snapshot/event feed** — stable JSON the agent reads each turn | S | Grounded, repeatable conversations. |

**Guardrails & decisions.** Ship **read-mostly first** (perception + suggestions); gate
`write_mem`/`run_lua` behind a confirm-before-write mode, since a bad poke can crash the core.
AI-discovered regions should write back into the ROM map as `::: region` blocks with an
`author: ai` / `confidence` provenance field, so they're distinguishable and reviewable — making
the AI's findings **durable across sessions**, not just chat.

### Core-capability matrix (verified live via MCP)

Whether the AI/RE workflow can touch bytes depends entirely on whether the loaded core publishes
its memory map. Measured by launching each core+ROM with `--mcp` and reading `app://memory-map` +
a Lua RAM scan:

| Core | System | ROM tested | Loads | `SET_MEMORY_MAPS` regions | Bytes readable | Notes |
|---|---|---|---|---|---|---|
| **fceumm** | NES | TMNT: Tournament Fighters | ✅ | **14** (RAM/PPUREG/NTARAM/PALRAM/OAM) | ✅ | **The working RE config** — full structured map incl. sprite RAM + palette |
| mesen | NES | TMNT | ❌ | — | — | Exits silently on load (bug, above) |
| genesis_plus_gx | Genesis | Mortal Kombat II | ✅ | **0** | ❌ | RAM only via `get_memory_data` (unconsumed) — needs the fallback above |
| fbalpha2012 | CPS2 | Marvel vs. Capcom | ✅ | **0** | ❌ | No map; FBNeo only adds work-RAM via `get_memory_data` |

**Takeaway:** NES (fceumm) is the cleanest core for the full ROM-DNA demo today — it publishes
OAM/PALRAM/nametables. Genesis and CPS2 are blocked on the `get_memory_data` fallback (Near-term).

### Convergent evidence — "ROM DNA" and the literate ROM

The deeper goal isn't a single tool that answers a question — it's giving Claude **multiple
independent lines of evidence that converge**, the way a real reverse-engineer (or a forensic
analyst) works. No one method is authoritative; agreement between methods is. For *"which ROM
holds the on-screen characters' sprite pieces,"* Claude can triangulate:

- **Vision** — read `app://screen`, *see* the rendered characters, and reason about what's there.
- **Content match (hex DNA)** — pull on-screen tile bytes from VRAM and `search_memory` them
  across ROM (the AI Wave 2 `vram_to_rom` primitive). Each tile is a fingerprint; a cluster of
  matches in one ROM span is a sprite-data block.
- **Image recognition** — render candidate ROM regions *as* tiles (decode ROM bytes with the
  system's pixel format) and visually compare the result to the on-screen character — the
  inverse of content match, and it survives some transforms that byte-comparison can't.
- **Structure** — major-region discovery: scan the ROM for the statistical signatures of code
  vs. graphics vs. tables (entropy, byte-histogram, tile-ness), proposing "this 512 KB span
  looks like packed sprite data." The PC heatmap + CDL-style code/data logging feed this.
- **Execution** — which code touches which VRAM, when (the DMA/control-port intercept on the
  roadmap), pinning the *loader* even when the data itself is compressed.

This is the **"natural DNA" of a ROM**: a tile, a palette, a sound table, a routine each leave a
recognizable signature, and the same character's sprites carry the same DNA in VRAM and in ROM.
Claude's job is to *tinker* — try a method, corroborate with another, and when they agree, write
the finding into the **literate ROM map** (`ROM_MAP_FORMAT.md`) as a confirmed region with its
evidence and `confidence`. Over many sessions the map accretes into a documented genome of the
ROM, co-authored by tool, human, and AI. The emulator, the live memory map, the ROM map, the
Lua probes, and the MCP surface are all instruments serving that one literate-documentation
end — which is exactly the surface area Claude is good at exploring.

**What this implies for the build order** (folds into the sub-track above): the content-match
primitive exists (Wave 2). The high-leverage additions are **(a) a ROM-region tile renderer**
(decode a ROM span as tiles → PNG, so vision/image-recognition can compare it to the screen —
✅ shipped, PR #9), **(b) major-region discovery** (entropy/histogram/tile-ness scan proposing
graphics vs. code vs. data blocks — ✅ shipped as `scan_regions`, PR #10), and **(c) the
DMA/execution provenance** hook (shared with the VDP source work — ⛔ spiked 2026-06 as
infeasible on stock cores; see Wave 4 above and `src/debug/vdp_source.rs`). Each is an
independent evidence stream Claude can cross-check — none has to be perfect, because
**convergence, not any single method, is what makes a finding confirmed.** With (c) ruled out on
stock cores, convergence rests on the (a)/(b)/content-match trio.

**(d) ROM-file source — the file-resident answer to "the core hides the graphics ROM"** (✅
shipped, PRs #15/#16). The DMA spike (c) and the core-capability matrix established that a running
core often won't expose the graphics ROM (NES CHR, CPS2 gfx) as live memory — but the **ROM file
itself is ours**. RustRetro retains the cart bytes (`DebugState.rom_bytes`/`rom_path`) and exposes
a `rom_file` source so `render_tiles`/`scan_regions` decode content the core never publishes. For
NES, an iNES/NES-2.0 parser (`src/mcp/ines.rs`) adds `rom_file:header/:prg/:chr` named spans and a
`rom_info` tool, so `render_tiles source=rom_file:chr` renders a cart's CHR graphics with no offset
math — verified live on TMNT (the core exposes nametables/OAM/palette but not the CHR banks). This
turns "core won't expose it" into a solved problem for file-resident data. Whole-CHR linear decode
works for all CHR-ROM mappers (bank-switching only selects the live window); CHR-RAM carts are the
exception (no CHR-ROM in the file). Genesis/CPS2 file-extraction is a separate, larger effort
(CPS2 is need_fullpath + encrypted/interleaved gfx). Future overlap to explore: ingesting Mesen/
FCEUX **CDL** (Code/Data Logger) as a runtime-truth code-vs-data-vs-graphics signal stronger than
the entropy-based `scan_regions` heuristic.

## Non-goals

- Becoming a general-purpose, configure-everything emulator frontend (RetroArch exists).
  Features earn their place by making a game easier to *take apart*, not just to play.
