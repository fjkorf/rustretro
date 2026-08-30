# RustRetro — working notes for Claude sessions

RustRetro is a debugging-instrument-first libretro frontend (see ROADMAP.md).
Per-game knowledge is DATA: `--game library/<game>` loads a two-tier profile
(family.json + <game>.profile.json — see docs/game-profiles.md; asurabld is
the complete reference, sf2ce a partial game-#2 seed, mk2 a boot stub).
Rust, Python (`shadow_train.profile`), and Lua (`game.*` bindings, API v3)
all read the same JSON — never hardcode a game address in code again.
MK2 note: runs on the from-source FBNeo core (../FBNeo) with readable RAM +
save states; `memory.cpu: "tms34010"` in its profile gates the Sek capture.
The active project is the **frame lab**: measured frame data (safe vs unsafe,
on hit and on block) for MK2 arcade — `docs/frames.md` is its contract, and
the section below is the short version. It sits on top of **shadow**: a
training mode + a behavioral-cloning opponent, originally for Asura Blade
(`shadow/PLAN.md`, `shadow/SPEC.md`). The literate
ROM map `library/asurabld/asurabld.md` is the EVIDENCE doc (how each value
was verified); the machine-readable extract is the profile JSON, which every
side reads — address changes go to the profile (+ the .md's evidence), never
into code.

## Build & test — use the fast profile

```sh
cargo build --profile release-dev   # ~3 s incremental — the dev loop
cargo test  --profile release-dev   # full suite
cargo build --release               # ~6 min (fat LTO) — SHIPPING ONLY
```

The 6-minute `--release` build is fat LTO re-merging 722 crates; never use it
for iteration. The dev binary is `target/release-dev/rustretro`.

## Running the game

```sh
./target/release-dev/rustretro \
  --core "$HOME/Library/Application Support/RetroArch/cores/fbalpha2012_libretro.dylib" \
  --rom ~/games/roms/asurabld.zip \
  --bus-map library/asurabld/asurabld.busmap.json \
  --training --script library/asurabld/training.lua \
  --mcp --mcp-port 4025 \
  --record shadow/recordings/asurabld/session-$(date +%Y%m%d-%H%M).jsonl \
  --shadow shadow/models/asurabld/goat-v2 --scale 3
```

Notable flags: `--headless` (agent-driven, implies `--mcp`), `--load-state
SLOT|PATH`, `--calibrate` (controller wizard → `keymap.json`), `--keymap`,
`--dump-keymap`, `--pad-debug` (raw button names to stderr), `--mute`,
`--no-audio`.

A family with more than one port picks it with a path segment, e.g. Genesis
MK2: `--game library/mk2/genesis --core
../FBNeo/src/burner/libretro/fbneo_libretro.dylib --rom
~/games/roms/genesis/md_mk2.zip` (arcade is `library/mk2`'s bare-dir default).
A profile's `pins` key (session pins) rewrites declared RAM values every
frame while loaded — MK2 Genesis uses it to hold the 6-button-mode flags.

### Hotkeys

| Key | Action |
|---|---|
| F1–F4 | training: cycle dummy / reset positions / toggle refill / finish round |
| F5 | toggle training mode |
| **Shift+F5** | toggle the native shadow (needs `--shadow`) |
| F6 / F7 (+Shift = slot 2) | save / load state slot |
| F8 / F10 / F11 | tutorials / Lua script panel / controls (rebind) |
| F12, Space | debugger, pause |

The F12 debugger groups panels into regions: Canvas (Frame/Disasm/Hex/Tiles,
center), Live (Watch/CPU/Input/**Input Log**, top right), Control (💾 State / 🎯 Training /
Audio, bottom right), Tools (bottom strip). The ☰ toolbar menu saves/resets
the layout and reopens closed panels; the sidecar is `rustretro_layout_v2.json`
(cwd, gitignored). Hotkey docs live in ONE place: `KEYBINDINGS` in
`src/main.rs` (rendered by the Help panel + printed at startup) — update it in
the same commit as any hotkey change.

Default keyboard/pad bindings, the F11 Controls panel, the `--calibrate` wizard,
`keymap.json`, and the Mayflash F300's required switch mode are all covered in
`docs/tutorials/controls.md` — read that instead of hand-tracing `keymap.json`.

## The shadow loop

```sh
shadow/loop.sh              # fit goat-vNEXT from recent recordings, drill list, fight
shadow/loop.sh --fit-only   # just fit + coverage report
shadow/loop.sh --model NAME # fight an existing model
shadow/loop.sh --push       # fit, then load into the running app's NATIVE
                            # runner (MCP load_shadow; Shift+F5 to fight)
```

The whole lifecycle is also driveable from the 🎯 Training panel: recorder
start/stop (auto-named into `shadow/recordings/`), the loaded-model card with
per-bucket coverage (sparse buckets ⚠ = the drill list), and a model picker
that hot-loads any model dir (data roots are PER-FAMILY: `shadow/models/<family>/`, same for recordings/arenas — one game's data never appears under another). Runtime loads arrive DISABLED
unless a shadow was already enabled; `--shadow` startup loads stay
enabled-and-fatal-on-error. The MCP `load_shadow` tool is the scripted twin
(gated behind `enable_writes`).

Matchups: models can be per-matchup — `shadow/loop.sh --me 1 --opp 7` fits a
`goat-vs-rosemary-vN` (slugs from the family's own `family.json` roster; ids
in asurabld.md's roster table). `python -m shadow_train coverage` prints the
matchup matrix (decisions per me×opp cell, demo-filtered). The recorder
writes a `.rounds.jsonl` sidecar per recording (one summary line per round:
matchup chars, frames, demo-ness, style) — the cheap index for coverage
tooling (`python -m shadow_train index` backfills old recordings); the
panel's Record section takes an optional style tag ("rushdown", "zoning")
stored in the sidecars and selectable at fit time.

The 🥊 Matchup panel renders that index as a coverage grid (≈decisions per
me×opp cell, ✓ = fitted model, amber = sparse); clicking a cell offers its
model, its `shadow/arenas/<slug>.state` arena, the fit command for gaps, and
"⚔ Force next fight" — freezes `$40364D` (write-verified opponent+venue
selector, asurabld.md "Stages") so every next fight is that matchup until
cleared. Bosses get quick-force buttons; Footee has no selector value.
Model SETS: loading a directory of model dirs (e.g. `shadow/models` via the
panel's "Load ALL as set" or `load_shadow`) keeps the newest model per
matchup key and auto-switches at every round start by reading both char ids
— fallback exact → per-char → per-opp → general.

Training saves: the panel's Arena section lists `shadow/arenas/*.state`,
loads one, captures the on-screen situation as a new named arena, or promotes
one to `shadow/arenas/current.state` — the pointer loop.sh starts fights from
(fallback: ARENA env → current.state → goat-vs-rosemary.state, under
`shadow/arenas/<family>/`; loop.sh takes FAMILY env, default asurabld). current.state
is gitignored; named arenas are committable.

Python side: `shadow_train` is `pip install -e`'d into `shadow/train/.venv`
(works from any cwd): `python -m shadow_train fit|eval|report`. `fit`/`eval`
auto-resolve their game+port profile from the recordings' own v3
`.meta.json` sidecars — no `RUSTRETRO_GAME_DIR` needed for a normal fit
anymore; `--game PATH` (or `RUSTRETRO_GAME_DIR`) still works as an explicit
override and warns loudly if it disagrees with the sidecars. The deploy
alternatives are the native runner (Shift+F5, in-app) and `shadow/play.py`
(over MCP; `--dry-run` observes safely against a live session). Recordings
are jsonl-v2/v3 (v1 files are rejected); training filters demo rounds by
zero-`p1_input`. The kNN uses a fit-time neutral cap + soft retrieval —
without both, the shadow stands still (absorbing-state failure, documented
in `shadow/train/shadow_train/dataset.py`).

## MCP / agent workflow

- Convention: the user's session runs on port **4025** — never kill it, never
  inject on it without dry-run/consent; agents launch their own headless
  instances on 4026+.
- Python client: `from shadow_train.mcpclient import McpClient` (handles the
  initialize handshake — raw POSTs without it get 422).
- `enable_writes` is **per-MCP-session**; it also arms the Lua write gate.
- Injected input drains on GUI frames — it evaporates while paused. For
  frame-exact work: `pause` → `step`/reads → `resume`.
- WRAM snapshot via bus window costs ~10 ms; snapshot-diff + pause/step is
  the effective RE discovery method (see asurabld.md for the session log).

## The frame lab — measured frame data

`docs/frames.md` is the NORMATIVE contract for measured frame data (startup /
active / recovery / advantage on hit and on block). Read it before touching
anything that measures. The authoring store is SQLite under
`shadow/framelab/` (gitignored, Python-owned, stdlib `sqlite3` — Rust never
opens it); the COMMITTED artifact is `library/<family>/<port>.frames.json`.
Harness: `shadow_train.framelab` (`session` owns the preconditions, `probe`
the act-again measurement, `arenas` the spacing ladder, `kit` the runner).

Spacing is data: `shadow/arenas/mk2/gap-K.state` + `.gap.json` is a ladder in
walk-frames AND pixels (K=0 is the FARTHEST rung; walking cannot reach gap 0,
MK2 has a ~62 px collision floor). MK2 has proximity normals, so the same
button is a different MOVE by distance — gap is part of every row's key.

The 🎯 Training panel gained reversal timing (Fast/Delay/Late/explicit,
replacing a fitted `PUNISH_DELAY`) and record/playback input slots
(`shadow/inputs/<family>/`, MCP `record_inputs`/`play_inputs`); training
settings now persist to `rustretro_training_v1.json` (cwd, gitignored).

### Measurement laws (each one cost us a wrong number)

- **No ABSOLUTE observation of motion means anything.** On a game where
  things move on their own, run the identical scenario with and without the
  input and believe only the DIFFERENCE. This killed three separate designs:
  the first probe draft, the arena liveness probe (which reported a
  CPU-driven port as a live human port), and the `action_counter` contact
  claim.
- **Never anchor on a DRAWN value.** MK2's HUD health bar animates toward
  its target at 1 unit/frame, smearing one hit into 11 edges. Anchor on what
  the game computes (struct health `block+0x0E`), not what it displays.
- **Cross-method agreement buys precision, not truth.** Two observables
  agreeing to the frame on every sweep still published a number 9 frames
  wrong, because both shared one flawed subtraction. Only an INDEPENDENT RIG
  found it.
- **Absent is not zero.** One schema change from "synthesized 0" to "absent"
  exposed five consumers that had all been reporting success on frozen data.

## Gotchas

- **`step` is FIRE-AND-FORGET**: 30 rapid MCP `step` calls were measured
  landing **1** frame. Poll `get_state`'s `frame_count` until it advances
  before the next step — and then let the frame FINISH (~8 ms) before
  changing input, or ~1 run in 50 lands a hold a frame early.
- **`load_state` does NOT drain while paused.** Resume → load → verify
  against a known field → pause. Skipping the verify silently measures the
  PREVIOUS state.
- **`press_buttons` decays on GUI frames** (including while paused) — it is
  BANNED in the frame lab; use `hold_buttons`/`release_buttons`.
- MK2 arcade positions live behind a POINTER (`block-0x0C`, TMS34010
  bit-address) because the object-pool slot moves at runtime; the profile's
  `via: "object_ptr"` field form resolves it per frame and yields ABSENT,
  never 0, when the pointer is stale. The old `p1_x`/`p2_x` globals are
  DISPROVEN — never use them for position or liveness.
- MK2 walk speeds are ASYMMETRIC (+12 px/6f forward vs +5 px/6f backward), so
  any check shaped "hold a direction, undo it, expect to return" is wrong.
- The app rewrites `library/asurabld/asurabld.busmap.json` (pretty-printed)
  on sidecar saves — don't commit that churn.
- Block `+0x54/+0x56` positions are recomputed outputs: writes hold only
  while paused (position reset = save-states or world-X fields).
- Health regenerates standing still; the combo counters linger after combos
  (hitstun = counter *changed* within 20 frames).
- The Lua sandbox has no `io`/`os`; `LuaEngine::reload()` destroys VM state
  (in-memory recordings die on reload — mutate `CONFIG` via `run_lua`).
- `scripts/re/revenv` exists solely for capstone (`execmap.py`); everything
  else uses `shadow/train/.venv`.
- `shadow/models/` and `shadow/recordings/` are gitignored (derived/data);
  `goat-v2` stays tracked because `shadow_runner` tests read it.
