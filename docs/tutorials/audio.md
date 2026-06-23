---
page:
  name: Audio
  label: "Audio"
  default: false
---
<!-- Once mounted in litui, this page inherits shared styles via parent: "_tutorials.md". -->

# Audio

**What you'll do:** mute the game or set its volume from the debugger.

Open the **🔊 Audio** tab.

## Steps

1. **Mute** — tick the checkbox to silence output instantly; untick to restore it.

2. **Volume** — drag the **Volume** slider (0–100%). The current percentage is shown
   below it.

3. The panel also reports the **Sample Rate** and whether audio is **Enabled** or
   **Disabled** (it's disabled if you launched with `--no-audio`).

## Why it matters

Long RE sessions mean a lot of looping menu music and repeated hit sounds. A one-click
mute keeps you sane without quitting the emulator or touching your OS mixer.

## See also

- [Getting Started](/docs/tutorials/getting-started.md) — the `--no-audio` launch flag.
- [The Docking Workspace](/docs/tutorials/docking-workspace.md) — dock the Audio tab wherever suits you.

<!-- litui:live
This is the most natural litui-native page: mute/volume are pure list/form/display, so when litui is
integrated the panel can be authored entirely with standard widgets — a [checkbox] for Mute, a [slider]
for Volume — bound to AppState (no [custom] escape hatch needed). A [display] can show the live Sample
Rate and Enabled/Disabled status (live-resource binding). Until then it renders as a static document page.
-->
