---
page:
  name: DisassemblyAndBreakpoints
  label: "Disassembly & Breakpoints"
  default: false
---
<!-- Once mounted in litui, this page inherits shared styles via parent: "_tutorials.md". -->

# Disassembly & Breakpoints

**What you'll do:** read live M68K code at the program counter, set breakpoints,
single-step, and run-to-a-line.

Open the **📜 Disasm** panel. It decodes the bytes at the M68K PC with Capstone.

## Steps

1. **Follow the PC.** The header shows **M68K PC: $……**. The disassembly lists the
   instructions there, with the current instruction marked by a **→** arrow and tinted
   green. If a Z80 is active, its PC is shown too.

2. **Pause and step.** Click **⏸ Pause** (or press **Space**). Once paused, **▶ Step**
   runs a single instruction; click it repeatedly to walk the code. **▶ Resume** lets
   it run again.

3. **Set a breakpoint.** Click the **⚫** dot to the left of any instruction — it turns
   into a red **🔴** and the line tints red. When execution reaches it, the panel shows
   **🔴 BREAKPOINT HIT at $……** and the emulator pauses. **Dismiss** clears the banner;
   **Clear BPs** removes all of them (up to 8 breakpoints at once).

4. **Run to a line.** Right-click an instruction and choose **▶ Run to here** (or just
   right-click the line). The emulator resumes and pauses when it reaches that address —
   a one-shot breakpoint for "get me to this point."

5. **Label a range.** Right-click → **🏷 Label range starting here…** opens an inline
   form: set the **End** address, a **Label** (e.g. `damage_calc`), and a color, then
   **✅ Add Region**. Labeled ranges show a colored banner and tint in the disassembly,
   and appear in the **🗺 Regions** panel.

6. **Jump anywhere.** Use the toolbar **Go to:** field, or click a **→** in the Watch,
   Search, Regions, or Heatmap panels — the Disasm view focuses that address even when
   it's away from the live PC.

## Honest limit — where the code comes from

Disassembly needs the core to expose its code bytes. For arcade cores it pulls 256
bytes at PC via `SekFetchByte`; otherwise it walks the `SET_MEMORY_MAPS` regions. If a
core exposes neither, you'll see *"No code bytes — core does not expose memory via
SekFetchByte or SET_MEMORY_MAPS."* That's correct, not a bug. Workaround: read the
bytes by hand in the [Hex Dump](hex-dump.md) at the PC shown in the
[CPU Registers](cpu-registers.md) panel.

## Why it matters

Breakpoints and single-stepping turn "this PC touched my health address" into "this
exact instruction stores the damage" — the difference between a guess and a fix.

## See also

- [Tracking Changes](tracking-changes.md) — get a PC lead before you breakpoint.
- [Regions, Heatmap & Bookmarks](regions-heatmap-bookmarks.md) — manage the ranges you label.

<!-- litui:live
When litui is integrated, this page gains live embeds:
- [custom](disasm_slot) the live disassembly listing — a custom-painted instruction view with the green PC arrow,
  breakpoint dots, and right-click run-to-here; a bespoke spatial surface mounted via the escape hatch (escape hatch)
- [display] the live "M68K PC: $……" header beside step 1 (live-resource binding)
Until then it renders as a static document page.
-->
