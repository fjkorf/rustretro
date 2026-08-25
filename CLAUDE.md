# RustRetro — working notes for Claude sessions

RustRetro is a debugging-instrument-first libretro frontend (see ROADMAP.md).
The active project is **shadow**: a training mode + a behavioral-cloning
opponent for Asura Blade (`shadow/PLAN.md`, `shadow/SPEC.md`). The literate
ROM map `library/asurabld/asurabld.md` is the source of truth for all game
addresses — copies are hand-kept in `src/record.rs`, `library/asurabld/
training.lua`, and `shadow/train/shadow_train/asurabld.py`; change all four
or none.

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
  --record shadow/recordings/session-$(date +%Y%m%d-%H%M).jsonl \
  --shadow shadow/models/goat-v2 --scale 3
```

Notable flags: `--headless` (agent-driven, implies `--mcp`), `--load-state
SLOT|PATH`, `--calibrate` (controller wizard → `keymap.json`), `--keymap`,
`--dump-keymap`, `--pad-debug` (raw button names to stderr), `--mute`,
`--no-audio`.

### Hotkeys

| Key | Action |
|---|---|
| F1–F4 | training: cycle dummy / reset positions / toggle refill / finish round |
| F5 | toggle training mode |
| **Shift+F5** | toggle the native shadow (needs `--shadow`) |
| F6 / F7 (+Shift = slot 2) | save / load state slot |
| F8/F9/F10 | tutorials / litui / Lua script panel |
| F12, Space | debugger, pause |

The F12 debugger groups panels into regions: Canvas (Frame/Disasm/Hex/Tiles,
center), Live (Watch/CPU/Input, top right), Control (💾 State / 🎯 Training /
Audio, bottom right), Tools (bottom strip). The ☰ toolbar menu saves/resets
the layout and reopens closed panels; the sidecar is `rustretro_layout_v2.json`
(cwd, gitignored). Hotkey docs live in ONE place: `KEYBINDINGS` in
`src/main.rs` (rendered by the Help panel + printed at startup) — update it in
the same commit as any hotkey change.

Keyboard P1: arrows + Z/X/A/S (attacks L/M/·/·), Enter=Start, Shift=coin.
P2: IJKL + G/T/H, M=Start, N=coin. The Mayflash F300 fightstick must be in
**PS3-DInput + DPad** switch mode (mapping in `keymap.json`; recalibrate with
`--calibrate`, inspect with `--pad-debug`).

## The shadow loop

```sh
shadow/loop.sh              # fit goat-vNEXT from recent recordings, drill list, fight
shadow/loop.sh --fit-only   # just fit + coverage report
shadow/loop.sh --model NAME # fight an existing model
```

Python side: `shadow_train` is `pip install -e`'d into `shadow/train/.venv`
(works from any cwd): `python -m shadow_train fit|eval|report`. The deploy
alternatives are the native runner (Shift+F5, in-app) and `shadow/play.py`
(over MCP; `--dry-run` observes safely against a live session). Recordings
are jsonl-v2 (v1 files are rejected); training filters demo rounds by
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

## Gotchas

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
