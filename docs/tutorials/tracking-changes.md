---
page:
  name: TrackingChanges
  label: "Tracking Changes"
  default: false
---
<!-- Once mounted in litui, this page inherits shared styles via parent: "_tutorials.md". -->

# Tracking Changes — Who Wrote This Value?

**What you'll do:** use a watch's **🔍 Track** toggle to log every frame an address
changes, along with the program counter running that frame — your lead on the code
that writes it.

## Steps

1. Add the address as a watch (see [Watch & Freeze](/docs/tutorials/watch-and-freeze.md)). For
   example, watch your health address from the [RAM Search](/docs/tutorials/ram-search.md) hunt.

2. In the **👁 Watch** grid, tick the **🔍 Track** checkbox on that row.

3. Make the value change — take a hit, gain meter, whatever moves it.

4. Expand the **🔍 Change Log (N)** section at the bottom of the Watch tab. Each
   logged change shows four columns:
   - **Frame** — the frame the change was detected on.
   - **Addr** — which tracked address changed.
   - **old → new** — the values before and after.
   - **PC** — the M68K program counter sampled on that frame.

5. Take the **PC** value into the **📜 Disasm** panel: type it into the toolbar's
   **Go to:** field, or use the row **→** elsewhere, and read the instructions around
   it. That's your starting point for finding the routine that touches this address.

6. Use **🗑 Clear** to empty the log between experiments.

## Honest limit — it's frame-granular

The logged **PC** is sampled *per frame*, not at the exact write. The actual store
happened *sometime during* that frame, not necessarily at that instruction. Treat the
PC as a neighborhood to investigate, not a pin on the exact `MOVE`. To pin it down
further, set a breakpoint nearby and single-step (see
[Disassembly & Breakpoints](/docs/tutorials/disassembly-and-breakpoints.md)).

## Why it matters

Knowing *where* health lives is half the job; knowing *who writes it* is the other
half. The damage routine it points at is the one you'll hook for a damage-scaling
tweak, a hitbox overlay, or a deeper RE.

## See also

- [Disassembly & Breakpoints](/docs/tutorials/disassembly-and-breakpoints.md) — pin the write with a breakpoint.
- [RAM Search](/docs/tutorials/ram-search.md) — how to find the address in the first place.

<!-- litui:live
When litui is integrated, this page gains live embeds:
- [display] the live "🔍 Change Log (N)" rows (Frame / Addr / old→new / PC) beside step 4 (live-resource binding)
- [custom](changelog_slot) the change-log table with its Track toggle and Clear button (escape hatch)
Until then it renders as a static document page.
-->
