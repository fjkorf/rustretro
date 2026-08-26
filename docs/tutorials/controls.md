---
page:
  name: Controls
  label: "Controls"
  default: false
---
<!-- Once mounted in litui, this page inherits shared styles via parent: "_tutorials.md". -->

# Controls

**What you'll do:** read and rebind your controls in the 🎛 Controls panel, understand
where the action names come from, and know when to reach for the CLI wizard instead.

## The layer model

Only the physical→RETRO wiring is stored — a key or pad button mapped to one or more of
the 12 RETRO joypad bits (`keymap.json`). Everything else is resolved for display:
action names are built at render time by chaining the game profile's vocabulary, then
the core's own input descriptors, down to the raw RETRO bit name, so the same stored
map can show up as "Light" for Asura Blade without any UI code knowing that word.

## The 🎛 Controls panel (F11)

F11 opens a floating window listing every action for both ports as a grid: **Action**
(profile/descriptor name plus the RETRO bits it fires, e.g. `Toss (B+A+Y)`), **Keyboard**,
**Gamepad**. An action name in amber means it currently has no binding on either device.

- **Click-to-rebind**: click a binding cell, then press the new key or pad button.
  **Esc cancels** with no change. While a capture is armed, that press stops reaching
  the game for the frame — it only completes the rebind.
- **Excluded from capture**: `Escape` (reserved for cancel), `F1`–`F35` (app hotkeys,
  including F11 itself), `Space` (pause), `B` (the bookmark hotkey), and the `Shift` /
  `Control` / `Alt` / `Super` modifiers. The built-in default still binds `Shift` to
  Coin and that binding keeps working — it just can't be *re-created* through the
  panel; edit `keymap.json` by hand for that.
- **Stolen-binding warning**: rebinding replaces every existing control that fired the
  same chord for that action. If the physical control you pressed already belonged to
  a *different* action, a yellow inline warning names it, e.g. "South was bound to
  Light — overwritten."
- **Device-specific maps**: a **＋ device-specific map for `<name>`** button appears for
  the first connected pad that reads a device name and doesn't have one yet. It clones
  the generic gamepad map so a fightstick and a normal pad can diverge from there — a
  capture targeted at a device sub-row writes into that pad's map only; the generic
  `gamepad` map is the fallback for anything without its own entry.
- **Save (global keymap.json)** writes the resolved default location for every ROM.
  **Save for this game (`<rom>.keymap.json`)** writes a per-ROM sidecar, which loads in
  preference to the global file. **Revert to defaults** discards in-memory edits back to
  the built-in maps without touching disk.

## The CLI wizard (`--calibrate`)

`--calibrate` is the faster path for a fresh stick you don't want to drive through a
GUI yet: it prompts on stderr, one step per action, and captures the very next gamepad
button you press — no hunting for the right grid cell. Esc skips a step; pressing a
button already assigned to an earlier step is rejected ("already assigned — press a
different button"). It writes the global `keymap.json` and exits; relaunch without
`--calibrate` to play.

The step list is no longer hardcoded to Asura Blade — it's generated from the same
action vocabulary the panel uses (directions are skipped; the d-pad/lever passthrough
covers those). For Asura Blade that's Start, Coin, then the profile's attack classes in
family order: Light, Medium, Heavy, Launcher, Toss. A different game profile gets a
working wizard for free, in whatever order its `family.json` lists its attack classes.

## `keymap.json`

Resolution order at startup: `--keymap PATH` (a parse error here is fatal) →
`<save-dir>/<rom>.keymap.json` → `<save-dir>/keymap.json` → the built-in default maps.
`--dump-keymap` prints whatever resolved, as the starting point for a hand edit.

Schema v2 highlights:

- Keyboard values are **chord lists**, not single buttons — one key can fire several
  RETRO bits at once (same mechanism the F300 default uses for Toss and Launcher).
- `gamepad_by_device` keys a whole map by the pad's reported device name (what
  `--pad-debug` prints), with `gamepad` as the generic fallback for anything else.
- v1 files — keyboard values written as bare button names — still load unchanged; a
  bare name deserializes as a one-button chord.

The RETRO 12-bit mask stays the wire format everywhere downstream of this file:
recordings, the shadow bot, Lua, MCP.

## Where names come from

Action names resolve through three tiers, most specific first: the game profile's
attack classes and their button chords, then the core's own
`RETRO_ENVIRONMENT_SET_INPUT_DESCRIPTORS` labels (FBNeo sends these per game;
fbalpha2012 sends none), then the raw RETRO button name as a last resort. A button
neither the profile nor the core names anything for is left off the list entirely —
that silence is itself evidence the game never reads it.

## The Mayflash F300

The built-in default gamepad map assumes the F300 fightstick is in **PS3-DInput +
DPad** switch mode: top row L/M/H → South/West/North → B/A/Y, weapon-toss chord on
`RightTrigger` (B+A+Y), launcher chord on `Mode` (B+A), coin on `LeftTrigger`, Start on
both trigger-2 buttons, directions on the d-pad. Recalibrate with `--calibrate` if a
stick differs from this, and use `--pad-debug` to see the raw button names it reports.

## Why it matters

Every human-facing surface — this panel, the wizard, Help, the Input monitor — renders
the same action list, so what you rebind here is what every other view of "what does
this button do" agrees on.

## See also

- [Getting Started](/docs/tutorials/getting-started.md) — the toolbar and dock panels this
  panel lives alongside.
- [Input & Triggers](/docs/tutorials/input-and-triggers.md) — reading the live input state
  once your bindings are set.

<!-- litui:live
When litui is integrated, this page gains live embeds:
- [custom](controls_grid_slot) the real 🎛 Controls grid (per-port action rows, click-to-rebind,
  device-map sub-rows) as an escape hatch, replacing the static bullet descriptions above
- [display] the resolved keymap source path (--keymap / sidecar / global / default) beside
  the keymap.json section
Until then it renders as a static document page.
-->
