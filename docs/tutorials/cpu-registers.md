---
page:
  name: CpuRegisters
  label: "CPU Registers"
  default: false
---
<!-- Once mounted in litui, this page inherits shared styles via parent: "_tutorials.md". -->

# CPU Registers

**What you'll do:** read live M68K and Z80 register state, with changed registers
highlighted each frame.

Open the **🔧 CPU** tab.

## Steps

1. **M68000.** The header shows **PC** and **SR** (status register) plus the current
   frame number. Below are the data registers **D0–D7** and address registers
   **A0–A7**, each as a 32-bit hex value.

2. **Watch the deltas.** Any register that changed since the previous frame is tinted
   yellow; unchanged ones stay light gray (the PC highlights yellow when it moved too).
   Pause and **Step** in [Disassembly](/docs/tutorials/disassembly-and-breakpoints.md) and watch which
   registers light up per instruction — that's the data flow, made visible.

3. **Status flags.** The **Status Register Flags** section breaks SR into `T S M I`
   (trace, supervisor, master, interrupt level) and `X N Z V C` (the condition codes).
   Useful when stepping through a comparison/branch to see exactly which flag a branch
   tested.

4. **Z80.** The bottom section shows the Z80 PC and the `BC` / `DE` / `HL` register
   pairs — the audio coprocessor on Genesis hardware.

> CPU state is captured every frame.

## Why it matters

When you single-step the damage routine, the yellow-highlighted registers tell you
which one holds the damage amount, which holds the target pointer, and which flag the
hit/block branch turns on — turning raw instructions into understood logic.

## See also

- [Disassembly & Breakpoints](/docs/tutorials/disassembly-and-breakpoints.md) — step the code that moves these registers.
- [Regions, Heatmap & Bookmarks](/docs/tutorials/regions-heatmap-bookmarks.md) — snapshot a register state into a bookmark.

<!-- litui:live
When litui is integrated, this page gains live embeds:
- [display] the live M68K D0–D7 / A0–A7 / PC / SR register [display]s beside steps 1–2, with the changed-register
  yellow tint driven per frame (live-resource binding) — this is the canonical instrument [display] case
- [display] the decoded SR flag row (T S M I / X N Z V C) and the live Z80 PC / BC / DE / HL pairs
Until then it renders as a static document page.
-->
