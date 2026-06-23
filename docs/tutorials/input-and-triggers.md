---
page:
  name: InputAndTriggers
  label: "Input & Triggers"
  default: false
---
<!-- Once mounted in litui, this page inherits shared styles via parent: "_tutorials.md". -->

# Input & Triggers

**What you'll do:** read a 120-frame input history and set pause triggers that stop the
emulator at exactly the moment you care about — the bread-and-butter of frame work.

Two panels: **🕹 Input** (history) and **⏸ Triggers** (pause conditions).

## The Input Monitor (🕹 Input)

1. Open the **🕹 Input** tab. **Live Buttons** lights up green for whatever is held
   right now across the 12 buttons (`B Y SEL START ↑ ↓ ← → A X L R`).

2. **Last Press (frame #)** shows the most recent frame each button was pressed — quick
   confirmation a button registered.

3. The **Input Timeline** is a scrolling grid of the last frames (history holds up to
   120), one row per frame, one green cell per pressed button. Read a motion
   (`↓ ↘ → + punch`) off the timeline frame-by-frame.

## The Triggers panel (⏸ Triggers)

4. Open the **⏸ Triggers** tab. **Manual Control** mirrors the toolbar: **⏸ Pause /
   ▶ Resume** and **⏭ Step 1 Frame**, with a live `PAUSED / Running @ frame N` readout.

5. **Pause at Frame** — type a frame number, click **Set**, and the emulator pauses
   exactly when `frame_count` reaches it. Great for landing on a known event ("the
   super flash always starts at frame 1820").

6. **Pause When Pixel Changes** — enter an `X` / `Y` (use the
   [Frame Inspector](/docs/tutorials/tiles-and-frames.md) pixel picker to read coordinates) and **Set**.
   The run loop pauses the frame that pixel changes — e.g. watch a health-bar pixel and
   stop on the exact frame damage registers.

7. **Pause on Button Press** — pick a button and **Pause on next press** to stop the
   instant that input is read. Use **Clear** on any trigger to disable it.

   > Triggers are checked in the emulation run loop each frame.

## Why it matters

Fighting-game work is frame work: startup, active, recovery. A frame trigger lands you
on the exact frame; a pixel trigger catches the precise moment a hit connects; the
timeline lets you read the inputs that got you there.

## See also

- [Tiles & Frames](/docs/tutorials/tiles-and-frames.md) — pick the pixel coordinates for a pixel trigger.
- [Disassembly & Breakpoints](/docs/tutorials/disassembly-and-breakpoints.md) — once paused on the frame, step into the code.

<!-- litui:live
When litui is integrated, this page gains live embeds:
- [display] the live "Live Buttons" / "Last Press (frame #)" readout beside step 1 (live-resource binding)
- [custom](timeline_slot) the scrolling 120-frame input timeline grid (custom-painted spatial surface, escape hatch)
- [custom](triggers_slot) the trigger controls (frame / pixel / button) with the PAUSED·Running @ frame N readout
Until then it renders as a static document page.
-->
