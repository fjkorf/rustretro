---
page:
  name: TrainingMode
  label: "Training Mode"
  default: false
---
<!-- Once mounted in litui, this page inherits shared styles via parent: "_tutorials.md". -->

# Training Mode — the Held-Fight Sandbox

**What you'll do:** hold a fight open indefinitely (credits, timer, health all pinned),
drive a practice dummy, and record your own play as training data for the shadow bot.

## Launch

```bash
./target/release-dev/rustretro \
  --core "$HOME/Library/Application Support/RetroArch/cores/fbalpha2012_libretro.dylib" \
  --rom ~/games/roms/asurabld.zip \
  --bus-map library/asurabld/asurabld.busmap.json \
  --training --script library/asurabld/training.lua
```

`--training` arms native enforcement immediately at boot (equivalent to starting and
pressing F5). `--game DIR` (default `library/asurabld`) selects which game profile
supplies the addresses training mode reads and writes — see
[Porting a Game](/docs/tutorials/porting-a-game.md) for other ports. `--script` is
optional and adds scripted extras (below); training mode itself needs only `--training`
and a bus window over the fighter blocks.

## Hotkeys

| Key | Action |
|---|---|
| F5 | toggle training mode on/off |
| F1 | cycle dummy: Free → Stand → Crouch → Jump → Block |
| F2 | reset positions |
| F3 | toggle health refill |
| F4 | finish round now |

## What enforcement does

While training mode is on, native code (not Lua) holds three things every frame,
straight from the loaded profile's `enforcement` values:

- **Credits** — topped up toward a target, never allowed to run out mid-session.
- **Round timer** — held at a fixed value so a round never times out.
- **Health** — refilled below a threshold so nobody gets KO'd out of the drill (toggle
  with F3 if you want KOs to happen).

This is enforced natively regardless of whether a Lua script is loaded — turning the
script off (or never loading one) does not disable training mode.

## The 🎯 Training panel

Open the debugger (`--debug` or F12) and find the **🎯 Training** tab:

- **Enabled (F5)** checkbox — same toggle as the hotkey; turning it on also turns on
  refill, matching F5's behavior.
- **Dummy (F1)** dropdown — Free (human/shadow drives port 1), Stand, Crouch, Jump,
  Block.
- **Health refill (F3)** checkbox, **↺ Reset positions (F2)** / **🏁 Finish round (F4)**
  buttons.
- **⏺ Record demonstrations** — Start/Stop a `.jsonl` recording without a `--record`
  flag; an optional style tag (e.g. `rushdown`, `zoning`) is stored in the recording's
  sidecar and later selectable when fitting a shadow model. See
  [The Shadow Loop](/docs/tutorials/shadow-loop.md).
- **👤 Shadow bot** — the loaded model's card (case count, round count, fit date),
  per-bucket coverage counts with a ⚠ flag on buckets far sparser than the model's
  best-covered one (the drill signal), an Enable/Disable toggle mirroring Shift+F5, and
  a model picker with **Load ALL as set** to load every `shadow/models/*` model
  together (see [The Shadow Loop](/docs/tutorials/shadow-loop.md) for what a "set"
  means).
- **🏟 Arena** — a list of `shadow/arenas/*.state` save states: **📂 Load** any of
  them, **📌 Make current** to promote one to `current.state` (the pointer
  `shadow/loop.sh` starts fights from when it exists), or capture the on-screen
  situation as a new named arena with **💾 Save arena**.

## training.lua's extras

Loading `library/asurabld/training.lua` via `--script` adds behavior beyond what native
training mode covers: a scripted port-1 dummy driver with a `replay` mode (record port-0
input into memory, then loop it back), hitstun tracking from the combo counters, and a
stat overlay. **Enforcement — credits, timer, health — is deliberately absent from the
script**: it asks `training.enabled()` / `game.controllable()` via the Lua API rather
than re-implementing the trio, so training mode works identically with or without the
script loaded. See [Lua Scripting](/docs/tutorials/lua-scripting.md) for the API.

## Why it matters

A shadow bot only learns from what you demonstrate. Training mode is what makes
demonstrating cheap: no coin-feeding, no clock pressure, no accidental KOs cutting a
recording short — just you, the dummy or another human, and the record button.

## See also

- [The Shadow Loop](/docs/tutorials/shadow-loop.md) — turn recordings into a fitted
  model and fight it.
- [The Matchup Grid](/docs/tutorials/matchup-grid.md) — see which matchups you've
  actually demonstrated.
- [Lua Scripting](/docs/tutorials/lua-scripting.md) — the `training.*`/`game.*` bindings
  training.lua reads.

<!-- litui:live
When litui is integrated, this page gains live embeds:
- [custom](training_panel_slot) the real 🎯 Training panel (enable/dummy/refill/reset/
  finish, record start/stop, shadow model card + picker, arena section) as an escape
  hatch, replacing the static bullet descriptions above
- [display] live training.enabled()/dummy()/refill() state beside the hotkey table
Until then it renders as a static document page.
-->
