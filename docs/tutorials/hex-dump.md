---
page:
  name: HexDump
  label: "Hex Dump"
  default: false
---
<!-- Once mounted in litui, this page inherits shared styles via parent: "_tutorials.md". -->

# Hex Dump

**What you'll do:** browse a memory region byte-by-byte, jump to an address, and watch
which bytes change in real time.

Open the **📋 Hex** tab.

## Steps

1. **Pick a region.** Use the **Memory:** combo to choose which region to view (work
   RAM, SRAM, ROM, etc.). The header below shows the region name, type, address range,
   byte count, and whether it's `[RO]` or `[RW]`.

2. **Read the dump.** Each row is `address  16 hex bytes  ASCII`. Non-zero bytes are
   white, zero bytes dimmed, so structure pops out at a glance.

3. **Jump to an address.** Type a hex address into the **Address:** field and press
   Enter (or click **→**). The view scrolls so that row is visible.

4. **Spot live writes.** Leave **highlight changes** ticked — any byte that changed
   since the previous frame is tinted amber. Run the game and watch a counter tick, a
   health value drop, or a structure fill in. (Switching regions resets the comparison,
   so you won't get false positives on the first frame after a switch.)

5. **Arrive here from elsewhere.** Clicking a **→** in Watch, Search, Regions, or the
   Heatmap, or using the toolbar **Go to:**, switches this panel to the region
   containing that address and scrolls to it automatically.

## Why it matters

When you've found a value but want to understand the *structure* around it — is health
part of a per-character block? what's the byte right before it? — the hex view with
amber change-tinting is how you map a struct by watching it move.

## See also

- [Watch & Freeze](watch-and-freeze.md) — pin a specific address out of the dump.
- [Disassembly & Breakpoints](disassembly-and-breakpoints.md) — read code bytes by hand when a core hides the disassembly.

<!-- litui:live
When litui is integrated, this page gains live embeds:
- [custom](hex_slot) the live hex dump (address / 16 bytes / ASCII) with amber change-tinting — a custom-painted,
  scrolling spatial surface that stays bespoke and plugs in via the escape hatch (escape hatch)
- [display] the live region header (name / range / byte count / RO·RW) beside step 1 (live-resource binding)
Until then it renders as a static document page.
-->
