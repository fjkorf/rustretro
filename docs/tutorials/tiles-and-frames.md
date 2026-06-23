---
page:
  name: TilesAndFrames
  label: "Tiles & Frames"
  default: false
---
<!-- Once mounted in litui, this page inherits shared styles via parent: "_tutorials.md". -->

# Tiles & Frames

**What you'll do:** inspect the rendered frame pixel-by-pixel, and slice it into 16×16
tiles you can pick apart.

These are two panels that pair naturally: **🖼 Frame** (the whole picture) and
**🧩 Tiles** (the picture cut into tiles).

## The Frame Inspector (🖼 Frame)

1. Open the **🖼 Frame** tab — it shows the current framebuffer. Press **F12** first if
   the overlay is closed; it reads "No frame yet" until emulation produces one.

2. **Zoom** with the slider or the **1× / 2× / 4×** buttons to magnify.

3. **Pick a pixel.** Hover over the image and the readout shows `(x,y) R:.. G:.. B:..`
   for the pixel under the cursor — exact game-pixel coordinates and color. This is how
   you find the `(x,y)` to feed a [pixel trigger](input-and-triggers.md) or a Lua
   `gui.drawPixel`.

4. **💾 Save PNG** writes the current frame to `frame_NNNNNN.png` in the working
   directory.

## The Tile Viewer (🧩 Tiles)

5. Open the **🧩 Tiles** tab. The frame is sliced into a grid of **16×16** tiles.
   Adjust **Zoom** (1–8×) and tick **Hide blank tiles** to drop the empty black ones.

6. **Click a tile** to select it. The right-hand **Selected Tile** pane shows it
   enlarged plus a scrollable list of every pixel's `(x,y) #RRGGBB` value.

## Why it matters

The pixel picker gives you the exact game-space coordinates that hitbox overlays and
pixel triggers need — and the tile view lets you eyeball sprite/character art straight
out of the live frame while you hunt for the RAM that drives it.

## See also

- [Input & Triggers](input-and-triggers.md) — pause when a picked pixel changes.
- [Lua Scripting](lua-scripting.md) — draw overlays in the same game-pixel space.

<!-- litui:live
When litui is integrated, this page gains live embeds:
- [custom](frame_slot) the live framebuffer image with zoom + pixel picker; [custom](tiles_slot) the 16×16 tile
  grid and selected-tile pane — both custom-painted spatial surfaces that stay bespoke and mount via the escape hatch
- [display] the picked-pixel readout (x,y) R:.. G:.. B:.. beside step 3 (live-resource binding)
Until then it renders as a static document page.
-->
