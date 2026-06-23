---
page:
  name: VdpRegisters
  label: "VDP Registers"
  default: false
---
<!-- Once mounted in litui, this page inherits shared styles via parent: "_tutorials.md". -->

# VDP Registers

**What you'll do:** read the Sega Genesis VDP hardware registers ($00–$17) decoded
into plain-English bitfields.

Open the **📺 VDP** tab.

## Steps

1. The panel lists all 24 VDP registers in a table: **Reg** (`$00`–`$17`), **Raw**
   (the byte), and **Decoded** (a human-readable description of the active bitfields).

2. Read across a row. For example:
   - **$01 Mode 2** decodes to things like `Display ON`, `VINT enable`, `DMA enable`,
     `V28 (NTSC 224 lines)`.
   - **$0C Mode 4** tells you `H40 (320px)` vs `H32 (256px)`, shadow/highlight, and
     interlace mode.
   - **$0F** is the auto-increment added to the VRAM address after each data-port write.
   - **$13/$14** combine into the DMA length; **$17** decodes the DMA type
     (68K→VDP transfer, VRAM fill, VRAM copy).

3. Use it to confirm video config at a glance — resolution, what planes/sprites point
   where in VRAM, whether interrupts and DMA are enabled.

## Honest limit — the live source isn't wired yet

Right now the panel **shows zeros**. The decode logic is real and unit-tested, but
there's no read-back path feeding it live values. That's a hardware fact, not a bug:
on a real Genesis the 24 VDP registers are **write-only** — the CPU writes them
through the control port and there is no read-back, so neither `SET_MEMORY_MAPS` nor
the cores' debug symbols expose the register file.

Wiring a live source means one of: intercepting control-port writes, reading the
core's internal `vdp_reg[]` array by pointer, or parsing a save-state. Until then,
treat this panel as a ready decoder waiting for input — every raw byte you *do* obtain
elsewhere can be understood by reading the matching row here.

## Why it matters

Even un-wired, the decoder is a quick reference for what each register byte *means* —
useful the moment you find VDP-related values via [RAM Search](/docs/tutorials/ram-search.md) or a
[Lua script](/docs/tutorials/lua-scripting.md), or trace control-port writes in the disassembly.

## See also

- [Tiles & Frames](/docs/tutorials/tiles-and-frames.md) — see the VRAM data the registers point at.
- [Hex Dump](/docs/tutorials/hex-dump.md) — inspect VRAM/CRAM blocks directly.

<!-- litui:live
When litui is integrated, this page COULD gain a live [display] of the 24 VDP register Raw/Decoded rows
(live-resource binding) — but only once the VDP read-back source is wired (see the "Honest limit" above:
the registers are write-only, so there is no live source yet). So this page stays static until BOTH litui
lands AND a VDP source is hooked up. The decode table itself remains a useful static reference meanwhile.
-->
