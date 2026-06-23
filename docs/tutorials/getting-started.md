---
page:
  name: GettingStarted
  label: "Getting Started"
  default: true
---
<!-- Once mounted in litui, this page inherits shared styles via parent: "_tutorials.md". -->

# Getting Started

**What you'll do:** boot a fighting game with the debugger open and find your way
around the toolbar and the dock panels.

## Steps

1. Build the frontend once:

   ```bash
   cargo build --release
   ```

2. Launch a CPS2 fighter with the debug overlay open from the first frame. The
   `--debug` flag opens the dock workspace on startup:

   ```bash
   cargo run --release -- \
     --core ./mame2003_plus_libretro.dylib \
     --rom ./mvsc.zip \
     --debug
   ```

   (Marvel vs. Capcom on MAME 2003-Plus — any CPS2 fighter works the same way.)

3. You can also toggle the overlay any time with **F12**, and pause the emulation
   with **Space**. The terminal prints both reminders on launch.

4. Look at the top toolbar — it's always visible above the panels:
   - **◀ Back / ▶ Fwd** — address-history navigation (jumps you between places you've visited).
   - **▶ Run / ⏸ Pause** and **⏭ Step / ⏯ Step Frame** — drive the emulation.
   - **Go to:** a hex field — type an address (e.g. `FF0000`) and press Enter to jump the
     Disasm and Hex panels there.
   - **PC: $……** — the live M68K program counter.
   - **💾 Save layout / ⟲ Reset layout** — persist or restore your panel arrangement.

5. The panels live in a draggable dock below the toolbar. The default layout puts
   **📜 Disasm** in the center, **🔧 CPU** top-right, **👁 Watch** / **🗺 Regions** below
   it, and a tabbed strip along the bottom (**📋 Hex**, **🖼 Frame**, **🧩 Tiles**,
   **🕹 Input**, **🧾 Log**, **⏸ Triggers**, **🔊 Audio**, **🔍 Search**, **📺 VDP**, **❓ Help**).
   Click any tab to bring its panel forward.

## Why it matters

A fighting-game RE session is mostly *pause, look, narrow, repeat*. Having the
overlay open from frame zero — with Step and Pause one key away — is the whole point
of using an instrument instead of a player.

## See also

- [The Docking Workspace](/docs/tutorials/docking-workspace.md) — rearrange and save your layout.
- [RAM Search](/docs/tutorials/ram-search.md) — your first real hunt: the health bar.

<!-- litui:live
When litui is integrated, this page gains live embeds:
- [display] the live M68K PC ($……) and frame number beside step 4's toolbar tour (live-resource binding)
- [custom](toolbar_slot) the real Run/Pause/Step toolbar row as an interactive control strip (escape hatch)
Until then it renders as a static document page.
-->
