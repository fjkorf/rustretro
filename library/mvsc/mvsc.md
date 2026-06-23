---
schema_version: 1

rom:
  name: "Marvel vs. Capcom: Clash of Super Heroes"
  system: cps2
  sha1: "ec87e167b253e14f3d7af848a83324473c11258c"   # of the mvsc.zip romset container
  crc32: "9302942e"                                   # of the mvsc.zip romset container
  size: 22699761

settings:
  scale: 3
  volume: 0.8
  muted: false
  breakpoints: []
  watches: []

meta:
  genre: fighting
  year: 1998
  developer: "Capcom"
  progress: "boot + attract observed; graphics RE blocked on core memory map"
  tags: [arcade, cps2, 2d-fighter, versus]
---

# Marvel vs. Capcom — map

## Overview

First exploration session, via the RustRetro MCP/AI surface against a live run (core:
`fbalpha2012`, ROM: `mvsc.zip`). The goal of this session: locate, in ROM, the source data
for the on-screen **character portraits** and **animated sprites**.

**Observed frame** (attract loop, frame 2357, user-paused, 384×224): the Marvel vs. Capcom
attract screen showing Wolverine. Captured to [assets/attract_wolverine_f2357.png]. Two
distinct kinds of character art are visible in this one frame, and they are the two storage
problems to solve:

1. A large **portrait** — a static, posed close-up of Wolverine's head/upper body inside an
   angular comic-panel frame (left of screen).
2. A smaller **sprite** — an in-action full-body Wolverine pose (right of screen), one cel of
   an animation that plays at a consistent framerate.

## Working theory of CPS2 graphics storage (to verify)

> This is the hypothesis driving the search, not yet confirmed against bytes (see Blocker).

- **Portraits** are large, static, single images. They are likely stored **together as a
  table/bank** — Wolverine's portrait probably sits *near the other characters' portraits*
  in ROM, in a contiguous "character portrait" region with a regular stride. Finding one
  portrait should reveal the whole gallery (a cross-ROM "pull all portraits" query, per the
  library's design goal).
- **Sprites** have a **more complex storage scheme**: a character animation is built from
  many **small tile/cel pieces** that are *composited* (assembled into a frame from a parts
  list) and *sequenced* with per-frame timing. So a sprite is not one image in ROM — it's
  (a) a bank of small graphics tiles, (b) an assembly/OBJ list describing which tiles compose
  each cel and at what offsets, and (c) animation/timing tables driving the sequence. These
  three layers are stored separately and reference each other.
- CPS2 graphics ROM is bit-plane / tile-encoded, so raw bytes will **not** match the rendered
  RGBA pixels directly — a content search of VRAM-against-ROM may miss unless decoded.

## Method (how we will confirm this once memory is reachable)

The convergent-evidence / "ROM DNA" workflow (see ROADMAP → AI-friendly interface):

1. **Vision** — read `app://screen`, identify what is on screen (done: Wolverine portrait + sprite).
2. **Enumerate on-screen objects** — `run_lua` a CPS2 object-RAM probe (template:
   `examples/cps2_oam_probe.lua`) to list the active sprite/OBJ entries and their tile refs.
3. **Pull the pixels** — `read_region`/`read_memory` the referenced VRAM/graphics tiles.
4. **Trace to ROM** — `vram_to_rom` / `search_memory` those tile bytes across ROM regions
   (content match) to get candidate ROM source addresses. For sprites, also locate the
   **OBJ/assembly list** and **animation tables** that sequence the tiles.
5. **Cluster** — for the portrait, once one is found, scan for the regular stride that reveals
   the adjacent **portrait gallery** (the other characters).
6. **Corroborate & record** — confirm with a second method (e.g. render a candidate ROM span
   as tiles and visually compare), then write each confirmed span as a `::: region` block
   below with its `confidence` and evidence.

## Regions

_(No confirmed regions yet — see Blocker. Region blocks will accumulate here as the bytes
become reachable and each finding is corroborated.)_

## Blocker (this session)

**Memory is not reachable on the current core.** `fbalpha2012` does **not** call
`RETRO_ENVIRONMENT_SET_MEMORY_MAPS`, so RustRetro receives **zero memory regions**. Verified
live via MCP:

- `list_regions` → `[]`
- `run_lua` with `memory.read_u8` across the full 24-bit space → **0 non-zero bytes** in 4096
  probes (every read returns 0). The Lua engine reads through the same empty region table.
- `run_lua` itself works (frame advances, `return` values surface, `_RUSTRETRO_API = 1`), and
  **vision works** (`app://screen` returns a valid PNG of the live frame).

So steps 2–5 above (anything that touches RAM/ROM/VRAM bytes) cannot run on `fbalpha2012`.
What works today: **vision** and **execution control**; what is blocked: **region-addressed
memory inspection**.

**Next step:** switch to a core that publishes a memory map for CPS2 (FBNeo is the leading
candidate — research in progress), or add a RustRetro fallback that hardcodes the documented
CPS2 RAM/ROM/VRAM region layout when a core doesn't publish one. Either unblocks the full
ROM-DNA workflow above; this map will then be filled in with confirmed portrait/sprite regions.
