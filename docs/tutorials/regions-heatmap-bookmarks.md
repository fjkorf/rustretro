---
page:
  name: RegionsHeatmapBookmarks
  label: "Regions, Heatmap & Bookmarks"
  default: false
---
<!-- Once mounted in litui, this page inherits shared styles via parent: "_tutorials.md". -->

# Regions, Heatmap & Bookmarks

**What you'll do:** let a PC heatmap reveal the hot code, label the ranges you
understand, snapshot machine states with thumbnails, and persist it all to a sidecar.

Open the **🗺 Regions** tab. It has three collapsible sections plus a toolbar.

## Discover code with the heatmap

1. Just play. The **🌡 PC Heatmap** fills automatically, counting how often each M68K
   address is the program counter.

2. Expand it to see the **top addresses** sorted by visit count, each with a colored
   heat bar (cool blue → hot red). The hottest addresses are your main loops and
   per-frame routines.

3. Type into **Filter address** (e.g. `02` or `0x04`) to narrow to a page. Click a
   row's **→** to jump Disasm/Hex there. **🗑 Clear Heatmap** resets the counts so you
   can isolate the code that runs during one specific action (e.g. start clearing, then
   throw one punch).

## Label what you figure out

4. When you've identified a hot range — say the game loop — label it. Either right-click
   it in [Disassembly](/docs/tutorials/disassembly-and-breakpoints.md) → **🏷 Label range starting
   here…**, or manage existing labels in the **🏷 Code Regions** section here. Each
   region has a label, start/end, and color; click a start address or **→** to jump to
   it, **🗑** to delete it.

## Bookmark a machine state

5. At an interesting moment (title screen, round start, a specific super), press **B**
   or click **📌 Bookmark now [B]**. A **🗂 Bookmark** is captured with a 64×48
   thumbnail, the frame number, the M68K PC, and a register summary (D0/D1/A6/A7).

6. Double-click a bookmark's label to rename it; click the notes line to annotate it.
   **🗑** deletes it.

   > Honest limit: bookmarks are *annotations*, not save states — the frontend has no
   > save-state/rewind. A bookmark records *what the state was*, it can't restore it.
   > Thumbnails are regenerated during play, not persisted.

## Persist it

7. Click **💾 Save** (in the toolbar) to write your bookmarks and code regions to a
   `<rom>.regions.json` sidecar next to your saves. The path is shown right there. It
   reloads automatically next session, so your map of the game survives.

## Why it matters

Reverse-engineering a fighter is cartography. The heatmap finds the roads, regions name
them, and bookmarks mark the landmarks — and the sidecar means you never start the map
from scratch twice.

## See also

- [Disassembly & Breakpoints](/docs/tutorials/disassembly-and-breakpoints.md) — where regions are created and shown.
- [CPU Registers](/docs/tutorials/cpu-registers.md) — the live registers a bookmark captures a snapshot of.

<!-- litui:live
When litui is integrated, this page gains live embeds:
- [display] the live top-addresses heatmap list (address + visit count) beside step 2 (live-resource binding)
- [custom](heatmap_slot) the heat-bar visualization (cool blue → hot red); [custom](bookmarks_slot) the bookmark
  cards with their 64×48 thumbnails — custom-painted surfaces via the escape hatch
Until then it renders as a static document page.
-->
