# ROM Map Format — Schema & Taxonomy (v1 draft)

> **Status:** Draft for discussion. This is the load-bearing spec for RustRetro's evolving
> ROM-map library. It defines the on-disk format, the controlled vocabulary of region kinds,
> the body-block grammar, and the round-trip rules. Everything else (library UI, index, gallery)
> builds on this. See `ROADMAP.md` → "litui integration" for context.

## 1. What a map is

One Markdown file per ROM that is an **evolving map of the ROM's contents** — co-authored by
the human (prose, history, theories) and the tool (settings, discovered regions, captured
assets), growing over many sessions. The `.md` is the **source of truth**; a separate library
index is a regenerable query cache derived from all maps (not specified here — it reads only
what this spec defines).

### Design principles

1. **Source of truth is the Markdown.** Human-readable, hand-editable, git-diffable.
2. **Machine/human zones are strictly separated** (see §6) so the tool never clobbers prose.
3. **Identity is the ROM content hash,** not the filename (§3).
4. **The vocabulary is controlled** (§5) so the same `kind` means the same thing in every ROM —
   this is what makes cross-ROM queries ("all title screens") possible.
5. **`:::` blocks are litui-native** — the body grammar reuses litui's container-directive
   syntax, so litui can later render maps directly.

## 2. Library layout

```
library/
├── _library.md              # parent frontmatter: default settings + shared styles (litui inheritance)
├── library.index.json       # derived query cache — NOT source of truth, regenerable
├── <rom-slug>/
│   ├── <rom-slug>.md         # the map (this spec)
│   └── assets/               # captured binaries: PNGs, palette dumps — referenced, never embedded
└── …
```

`<rom-slug>` is a filesystem-safe slug derived from the ROM name; the canonical key is the
content hash in frontmatter, so a slug rename never orphans a map.

## 3. Frontmatter (machine-managed)

YAML frontmatter holds identity, settings, and metadata. **The tool owns this block entirely**
and may rewrite it in place. Three required sections:

```yaml
---
schema_version: 1

rom:
  name: "Asura Blade"
  system: cps2                 # controlled: nes | megadrive | cps2 | …
  sha1: "8f2e…"                # canonical identity key
  crc32: "a1b2c3d4"            # secondary; matches MAME romset where applicable
  size: 8388608

settings:                      # RustRetro per-ROM settings set via menus (overrides _library.md)
  scale: 3
  volume: 0.8
  muted: false
  breakpoints: [0x0210E0]
  watches: [{ addr: 0xFF0040, label: "p1_hp", format: u8 }]

meta:                          # human-curated, free-form-ish descriptive metadata
  genre: fighting
  year: 1998
  developer: "Fuuki"
  progress: "boot + title mapped"
  tags: [arcade, 2d-fighter]
---
```

| Section | Owner | Lifecycle | Notes |
|---|---|---|---|
| `schema_version` | tool | bump on migration | drives upgrades (§8) |
| `rom` | tool | write-once at scaffold | `sha1` is the library key |
| `settings` | tool | overwrite-in-place | mirrors menu state; layered over `_library.md` defaults |
| `meta` | human (tool seeds) | append/edit | tool fills detectable fields at scaffold; human curates after |

`settings` and `meta` are deliberately separate: settings are small and overwritten; meta and
the map body grow unboundedly.

## 4. Body — typed region blocks + prose

The body interleaves **human prose** (anything outside a block) with **typed region blocks**.
A block is a litui-style `:::` fence whose opening line is machine-managed metadata and whose
inner content is human prose:

```markdown
## Boot sequence

The boot routine clears RAM, then jumps to the title handler. (← human prose, tool never edits)

::: region kind=title_screen id=tt01 addr=0x024000-0x025FFF capture=assets/title.png confidence=confirmed
Title tilemap, drawn by `title_draw` (see [[gl01]]). Uses palette [[pal_title]].
Sub-rip at 0x024100 is the "INSERT COIN" overlay. (← human prose, freely editable)
:::
```

### Block grammar (strict)

```
::: region <attr>=<value> <attr>=<value> …
<human prose — zero or more lines, may contain markdown, links, ![captures]>
:::
```

- Opening fence: literal `::: region` then space-separated `key=value` attributes.
- Values: bare token (`title_screen`, `0x024000-0x025FFF`, `confirmed`) or `"quoted string"`
  if it contains spaces.
- **The opening fence line is 100% tool-owned and safely rewritable by `id`.** The tool finds a
  block by its `id`, updates fence attributes (e.g. adds `capture=…` after a screenshot), and
  rewrites *only that line*.
- **Everything between the fences is 100% human-owned.** The tool creates a block (with a stub
  line of prose) and may delete a whole block, but never rewrites the inner prose.
- `:::` on its own line closes the block.

### Required attributes (all kinds)

| Attr | Meaning |
|---|---|
| `kind` | controlled vocabulary value (§5) |
| `id` | stable, unique-within-file slug; tool-assigned (`<prefix><n>`), human-renameable; cross-refs use it as `[[id]]` |
| `addr` | `0xHHHH` (point) or `0xSTART-0xEND` (range), in the relevant address space |

### Optional attributes (common)

| Attr | Meaning |
|---|---|
| `label` | short human name (defaults to `id` if absent) |
| `capture` | path to a captured asset under `assets/` (tool-managed) |
| `confidence` | `confirmed` \| `likely` \| `guess` (default `likely`) |
| `cpu` | `m68k` \| `z80` (for code kinds; default = system primary) |
| `format` | hardware data/pixel format (recommend a per-system controlled list later) |
| `space` | address space: `cpu` \| `rom` \| `vram` (default `cpu` for code, `rom`/`vram` for assets) |

Cross-references use `[[id]]` in prose (and `palette=<id>` style attrs), matching the linking
convention RustRetro/litui memory already uses.

## 5. Controlled vocabulary of `kind`

The enumerated set for v1. Each kind lists its **required-beyond-common** and notable optional
attributes. Adding a kind is a `schema_version` bump.

### Code (located in CPU space)
| `kind` | Extra attrs | Notes |
|---|---|---|
| `game_loop` | — | the main per-frame loop |
| `subroutine` | opt `calls=[id,…]`, `called_by=[id,…]` | a named routine |
| `interrupt_handler` | opt `vector` | vblank, etc. |
| `sound_driver` | — | the audio engine entry |

### Graphics (visual, capturable — `capture` recommended)
| `kind` | Extra attrs | Notes |
|---|---|---|
| `title_screen` | opt `palette=<id>` | full-screen title image |
| `background` | opt `palette`, `dimensions` | a background layer/scene |
| `tilemap` | opt `dimensions` | tile arrangement data |
| `character_sprite` | opt `palette`, `dimensions` | one character's sprites |
| `sprite_sheet` | opt `palette`, `dimensions` | a sheet/bank of sprites |
| `palette` | opt `count` (colors) | a color table |

### Audio
| `kind` | Extra attrs | Notes |
|---|---|---|
| `music_track` | opt `index`, `driver=<id>` | one song |
| `sfx_table` | opt `count` | sound-effect bank |

### Data
| `kind` | Extra attrs | Notes |
|---|---|---|
| `level_data` | opt `count`, `stride` | stage/map definitions |
| `text_table` | opt `encoding` | dialogue/string table |
| `lookup_table` | opt `stride`, `count` | generic data table |

> The vocabulary is intentionally small for v1. "Find all character sprites" or "pull all title
> screens" is a query over `kind`; comparable results across ROMs depend on these meaning the
> same everywhere — so resist freeform kinds. Propose additions as schema changes.

## 6. Round-trip rules (data-loss safety)

| Zone | Owner | Tool may | Tool may NOT |
|---|---|---|---|
| Frontmatter | tool | rewrite in place | — |
| Region fence line | tool | update attrs by `id`, add `capture` | reorder/lose human blocks |
| Region inner prose | human | create stub, delete whole block | rewrite existing prose |
| Prose outside blocks | human | append new blocks/sections at end | edit existing prose |

Plus: **atomic writes** (`.tmp` + rename, as today) and **git is the merge/backup layer**. The
tool reads the whole file, mutates only tool-owned spans located by `id`, and writes it back.

## 7. Captured assets

Visual/audio artifacts are **captured to `assets/` and referenced**, never embedded in Markdown.
A `title_screen` block's `capture=assets/title.png` lets the library render a cross-ROM gallery
of title screens **from the stored PNGs** without re-running every ROM. Captures are tool-owned;
deleting a block does not auto-delete its asset (leave GC to a later maintenance pass).

## 8. Versioning & migration

`schema_version` (frontmatter) gates compatibility. When the vocabulary or grammar changes, bump
it and provide a migration that upgrades older maps on open. Maps below the current version are
read-only until migrated, so queries never mix incompatible shapes.

## 9. Scaffold ("explore new")

Opening a ROM with no map stamps this skeleton (detectable fields pre-filled):

```markdown
---
schema_version: 1
rom: { name: "<detected>", system: <detected>, sha1: "<hash>", crc32: "<hash>", size: <n> }
settings: { scale: 3, volume: 0.8, muted: false, breakpoints: [], watches: [] }
meta: { genre: "", year: "", developer: "", progress: "new", tags: [] }
---

# <name> — map

## Overview

_(notes go here)_

## Regions

_(region blocks accumulate here as you explore)_
```

Every map starting from one template is what makes exploration paths grow consistently across
the whole library.

## 10. Open questions (for discussion)

- **Multiple ranges per block** — v1 allows one `addr` range; revisit if regions are commonly
  discontiguous.
- **Per-system `format` vocabulary** — freeform for v1; controlled lists per system would make
  format-based queries possible.
- **`id` stability vs. renaming** — renaming an `id` breaks `[[id]]` refs; should the tool offer
  an assisted rename that rewrites references?
- **Index spec** — what exactly the query cache extracts and the query surface (deferred to its
  own doc).
- **litui rendering** — region blocks as interactive cards depends on litui's runtime parser
  (the third litui driver requirement; see `../litui/knowledge/rustretro-showcase.md`).
