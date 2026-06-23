# RustRetro Tutorials

These are task-oriented walkthroughs, not reference docs — each one picks a single
feature, hands you the real buttons, and walks you through doing one concrete thing
with a real ROM running. They're written for taking a CPS2 / Mega Drive fighter apart
while it plays. Start with [Getting Started](/docs/tutorials/getting-started.md) and follow the
cross-links from there.

> Each tutorial is a litui page — the same Markdown renders as a GitHub doc today and,
> once litui is integrated (see [ROADMAP](../../ROADMAP.md)), as an in-app **Help → Tutorials**
> screen. Pages carry minimal `page:` frontmatter (hidden on GitHub) and share styles via
> the `_tutorials.md` parent; live-data spots are annotated with `<!-- litui:live -->` markers
> for when litui's `[custom]` escape hatch and live-resource binding land.

## Getting Started
- [Getting Started](/docs/tutorials/getting-started.md) — launch with `--debug`, the toolbar, the dock panels.
- [The Docking Workspace](/docs/tutorials/docking-workspace.md) — arrange panels, save/reset layout, linked navigation.

## Memory & Search
- [Watch & Freeze](/docs/tutorials/watch-and-freeze.md) — pin addresses, pick a format, freeze a value.
- [RAM Search](/docs/tutorials/ram-search.md) — the canonical "find the health-bar address" hunt.
- [Tracking Changes](/docs/tutorials/tracking-changes.md) — `🔍 Track` a watch to find the PC that wrote it.
- [Hex Dump](/docs/tutorials/hex-dump.md) — browse raw memory with changed-cell highlighting.

## Code & Execution
- [Disassembly & Breakpoints](/docs/tutorials/disassembly-and-breakpoints.md) — follow PC, set breakpoints, run-to-address.
- [Regions, Heatmap & Bookmarks](/docs/tutorials/regions-heatmap-bookmarks.md) — discover code, label it, snapshot states.
- [CPU Registers](/docs/tutorials/cpu-registers.md) — M68K/Z80 state with per-frame deltas.

## Graphics & I/O
- [Tiles & Frames](/docs/tutorials/tiles-and-frames.md) — the tile viewer and frame inspector.
- [VDP Registers](/docs/tutorials/vdp-registers.md) — the Genesis VDP bitfield decoder (and its honest limit).
- [Input & Triggers](/docs/tutorials/input-and-triggers.md) — input history and pause triggers for frame work.
- [Audio](/docs/tutorials/audio.md) — volume and mute.

## Scripting
- [Lua Scripting](/docs/tutorials/lua-scripting.md) — load a script, the v1 API, building a hitbox overlay.

## litui page map

For the future `define_markdown_app!` wiring — each tutorial file maps to one litui `page: name`
(`getting-started.md` is the single `default: true` page; `_tutorials.md` is the shared parent
frontmatter, not a page):

| File | litui `page: name` |
|------|--------------------|
| `getting-started.md` | `GettingStarted` (default) |
| `docking-workspace.md` | `DockingWorkspace` |
| `watch-and-freeze.md` | `WatchAndFreeze` |
| `ram-search.md` | `RamSearch` |
| `tracking-changes.md` | `TrackingChanges` |
| `hex-dump.md` | `HexDump` |
| `disassembly-and-breakpoints.md` | `DisassemblyAndBreakpoints` |
| `regions-heatmap-bookmarks.md` | `RegionsHeatmapBookmarks` |
| `cpu-registers.md` | `CpuRegisters` |
| `tiles-and-frames.md` | `TilesAndFrames` |
| `vdp-registers.md` | `VdpRegisters` |
| `input-and-triggers.md` | `InputAndTriggers` |
| `audio.md` | `Audio` |
| `lua-scripting.md` | `LuaScripting` |
</invoke>
