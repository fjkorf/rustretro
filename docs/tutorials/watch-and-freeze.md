---
page:
  name: WatchAndFreeze
  label: "Watch & Freeze"
  default: false
---
<!-- Once mounted in litui, this page inherits shared styles via parent: "_tutorials.md". -->

# Watch & Freeze

**What you'll do:** pin a memory address to the **👁 Watch** tab, read it live in the
format you want, and freeze it so the game can't change it.

## Steps

1. Open the **👁 Watch** tab. The add-form sits across the top.

2. Fill in the form:
   - **Address:** a hex address, e.g. `FF0844` (the `0x` prefix is optional).
   - **Label:** a human name, e.g. `p1_health`. Leave it blank and the address is used.
   - **Format:** pick how the bytes are read — `u8`, `s8`, `u16 LE`, `u16 BE`,
     `u32 LE`, `u32 BE`, `hex8`, `hex16`, or `hex32`. Genesis values are big-endian,
     so reach for `u16 BE` / `u32 BE` there.

3. Click **➕ Add Watch**. The address appears in the grid with its live **Value**
   updating every frame. You can edit the **Label** in place at any time.

4. To stop the game changing a value, tick its **Freeze** checkbox. The current value
   is captured and re-written every frame — this is your "infinite health" toggle.
   Untick it to release the value back to the game.

5. The **→** button on a row jumps the **📜 Disasm** and **📋 Hex** panels to that
   address (handy once you find the write site). The **✕** button removes the watch.

## Why it matters

Freezing is the fastest way to *confirm* you found the right address: freeze it, take a
hit, and if your health stops dropping you've got the real thing. It also turns a
known address into a practice tool — infinite meter, frozen timer, locked round count.

## See also

- [RAM Search](/docs/tutorials/ram-search.md) — how to *find* the address before you watch it.
- [Tracking Changes](/docs/tutorials/tracking-changes.md) — once watched, find the code that writes it.

<!-- litui:live
When litui is integrated, this page gains live embeds:
- [display] the live watch values table beside step 3 — each row's Value updating every frame (live-resource binding)
- [custom](watch_slot) the real add-watch form + Freeze/Track checkboxes as an interactive readout (escape hatch)
Until then it renders as a static document page.
-->
