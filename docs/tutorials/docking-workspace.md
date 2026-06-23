---
page:
  name: DockingWorkspace
  label: "Docking Workspace"
  default: false
---
<!-- Once mounted in litui, this page inherits shared styles via parent: "_tutorials.md". -->

# The Docking Workspace

**What you'll do:** arrange the debug panels into a workspace that suits your task,
save it, and use linked navigation to jump every address-aware panel at once.

## Arrange panels

1. The dock lives below the toolbar. Each panel is a tab with an icon: **🖼 Frame**,
   **📋 Hex**, **🧩 Tiles**, **🕹 Input**, **🧾 Log**, **⏸ Triggers**, **🔧 CPU**,
   **🔊 Audio**, **📜 Disasm**, **🗺 Regions**, **👁 Watch**, **🔍 Search**, **📺 VDP**,
   **❓ Help**.

2. **Drag a tab** by its title to move it. Drop it onto another region's edge to
   **split** the dock, or onto a tab strip to **group** it. Build the view you want —
   e.g. Disasm + CPU side by side, Watch and Search stacked below.

3. The default layout puts **Disasm** in the center, **CPU** top-right, **Watch /
   Regions** below it, and the rest in a tabbed bottom strip — a good starting point.

## Save and reset

4. When you like the arrangement, click **💾 Save layout** in the toolbar. It writes to
   `rustretro_layout.json` in the working directory and reloads automatically next
   launch.

5. Click **⟲ Reset layout** any time to snap back to the built-in default.

## Linked navigation

6. The address-aware panels (**Disasm**, **Hex**, **Regions**, **Watch**, **Search**)
   share one navigation cursor. Click a **→** in **👁 Watch**, **🔍 Search**, or the
   **🌡 Heatmap**/**🏷 Regions** lists, or type into the toolbar **Go to:** field — and
   **Disasm** and **Hex** both jump to that address in the same instant. The Hex panel
   even auto-switches to the region containing the target.

7. The toolbar **◀ Back / ▶ Fwd** buttons walk the history of places you've jumped to,
   and **PC: $……** plus **@ $……** show the live program counter and your current cursor.

> The Lua Script window (**F10**) is intentionally a separate floating window, not a
> dock tab — so it stays put regardless of your layout.

## Why it matters

A good RE layout means the answer is always on screen: jump from a Search hit and watch
Disasm, Hex, and CPU all land on the same address together — no hunting, no re-typing.

## See also

- [Getting Started](/docs/tutorials/getting-started.md) — first tour of the toolbar and panels.
- [Disassembly & Breakpoints](/docs/tutorials/disassembly-and-breakpoints.md) — the central panel most jumps land in.

<!-- litui:live
This page is pure prose about arranging the dock — it has no meaningful live embed.
The docking workspace itself is litui's own concern (the frame/navigation it owns),
not a [custom] slot or [display] binding inside this tutorial. It stays a static
document page.
-->
