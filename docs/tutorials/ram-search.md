---
page:
  name: RamSearch
  label: "RAM Search"
  default: false
---
<!-- Once mounted in litui, this page inherits shared styles via parent: "_tutorials.md". -->

# RAM Search — Find the Health Bar

**What you'll do:** narrow tens of thousands of RAM bytes down to the single address
holding player-1 health, then turn it into a frozen "infinite health" watch.

This is the canonical reverse-engineering hunt. Open the **🔍 Search** tab.

## Steps

1. **Pick where to look.** In **Memory:**, choose a work-RAM region. Set **Size:** to
   `8` or `16` (health is usually a small integer — start with `8`). Leave **signed**
   off; toggle **hex** if you'd rather think in hex.

2. **Start a baseline.** Click **🔄 New Search / Reset**. The candidate counter shows
   every address in the region — this is your starting universe.

3. **Take damage.** In the running game, let P1 eat one hit so health goes *down*.

4. **Narrow by what happened.** Set **Operator:** to **↓ Decreased** and click
   **🔍 Next**. Every address that didn't decrease is dropped. The green
   **N candidates** count plummets.

5. **Repeat.** Block for a moment (**= Unchanged**, **🔍 Next**), take another hit
   (**↓ Decreased**, **🔍 Next**). After two or three rounds you'll be down to a
   handful of addresses — usually under the 1000-row cap, so they list out with their
   live values.

   > Tip: if you know the exact value (say health is `48`), switch the operator to
   > **= Equal**, choose **Compare to: specific value**, type it, and search. The
   > **± Different by** operator (with a delta) catches values that drop by a fixed
   > amount each hit.

6. **Promote the winner.** Click **+Watch** on the row that tracks your health. It's
   added to the **👁 Watch** tab. The **→** button jumps Disasm/Hex there instead.

7. **Freeze it.** In the Watch tab, tick **Freeze** on that row — health stops
   draining. Confirmed and weaponised in one move.

## Operators at a glance

- Compares vs. a **specific value**: `= Equal`, `≠ Not equal`, `< Less`, `> Greater`.
- Compares vs. the **previous checkpoint**: `≈ Changed`, `= Unchanged`,
  `↑ Increased`, `↓ Decreased`, `± Different by <delta>`.

## Why it matters

Almost everything downstream — infinite health, finding the damage routine, building
a hitbox script — starts with an address you trust. RAM Search is how you earn that
trust without a memory map.

## See also

- [Watch & Freeze](/docs/tutorials/watch-and-freeze.md) — what to do with the address you found.
- [Tracking Changes](/docs/tutorials/tracking-changes.md) — find the instruction that writes it.

<!-- litui:live
When litui is integrated, this page gains live embeds:
- [display] the live "N candidates" count beside step 4 as it plummets each pass (live-resource binding)
- [custom](search_slot) the operator/size controls + candidate-row list as an interactive hunt panel (escape hatch)
Until then it renders as a static document page.
-->
