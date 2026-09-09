# RustRetro — Debugging Notes & libretro Gotchas

Hard-won lessons from bringing real cores (Nestopia, Genesis Plus GX, MAME 2003-Plus) up
against this frontend. These are the things that cost the most time; keep them here so the
next core doesn't cost the same.

## 1. libretro environment-callback constants are NOT sequential

The single biggest bug in this project's history: the FFI layer originally numbered the
`RETRO_ENVIRONMENT_*` command constants 1, 2, 3… sequentially. The real `libretro.h` values
are sparse and specific. Only `GET_SYSTEM_DIRECTORY = 9` was right by coincidence — which was
enough for the lenient Nestopia core to limp along, hiding the fact that the whole table was
wrong. MAME, which is stricter during init, crashed.

Correct values (verify any new one against `libretro.h`, never guess):

| Constant | Value |
|---|---|
| `SET_PIXEL_FORMAT` | 10 |
| `GET_VARIABLE` | 15 |
| `GET_LOG_INTERFACE` | 27 |
| `GET_SAVE_DIRECTORY` | 31 |
| `SET_SYSTEM_AV_INFO` | 32 |
| `SET_MEMORY_MAPS` | 36 |
| `GET_VFS_INTERFACE` | 65581 (`45 \| 0x10000`) |
| `RETRO_PIXEL_FORMAT_XRGB8888` | 1 (pixel-format enum, not 2) |

The `0x10000` (`RETRO_ENVIRONMENT_EXPERIMENTAL`) flag is why newer interfaces (VFS, LED) have
huge cmd values that look like bugs but aren't.

**Lesson:** a forgiving peer (Nestopia) can mask a broken protocol. Cross-check against the
authoritative header, and treat the strictest core (MAME) as your conformance test.

## 2. `GET_VFS_INTERFACE` returning `true` without a struct is a crash bomb

A core that gets `true` for VFS will immediately call function pointers in the struct you were
supposed to fill in. We return `false` so cores fall back to stdio file I/O. Only return `true`
once the interface is actually implemented.

## 3. ROM loading: `need_fullpath` decides the strategy

`RetroSystemInfo.need_fullpath` splits cores into two loading modes:

- **`need_fullpath = true`** (e.g. MAME): pass the ROM *path*; the core opens the file itself.
- **`need_fullpath = false`** (e.g. NES/Genesis): read the whole ROM into memory and pass the
  *data pointer*.

Get this wrong and `retro_load_game` fails or the core reads garbage.

## 4. Disassembly: where the code bytes come from

The Disasm panel decodes `DebugState.m68k_code_bytes` with Capstone. Those bytes are sourced,
in priority order:

1. **`SekFetchByte`** — MAME/FBAlpha export the symbol `_Z12SekFetchBytej`
   (`extern "C" fn(u32) -> u8`, a side-effect-free instruction fetch). When present, the
   frontend pulls 256 bytes at PC each frame directly from the core. This is the path that
   makes disassembly work for arcade cores.
2. **`SET_MEMORY_MAPS` regions** — if the core published a memory map, translate PC → host
   pointer via `region.ptr + offset + ((addr & ~disconnect) - addr_start)` and read there.

If a core exposes *neither* (some MAME/FBAlpha builds don't implement `SET_MEMORY_MAPS`, and
older cores lack `SekFetchByte`), the panel shows *"No code bytes — core does not expose memory
via SekFetchByte or SET_MEMORY_MAPS."* This is correct behavior, not a frontend bug: the code
is simply not reachable from the frontend. **Workaround:** read the bytes manually in the
📋 Hex tab at the PC shown in the 🔧 CPU tab.

## 5. Quiet your environment callback for MAME

MAME fires environment callbacks in a tight loop during init. Per-call `eprintln!` logging
floods stdout and slows boot to a crawl. Keep the env callback's match arms minimal and log
only the unhandled/interesting commands.

## Relocated from src/profile.rs:577-666 (loom lint F-01, ratified 2026-09-09)

── frame lab data (docs/frames.md) ─────────────────────────────────────────

`library/<family>/<port>.frames.json` is a MEASUREMENTS STORE (§6), not a
profile constant — exported by the Python harness
(`shadow_train.framelab`), never authored or edited by Rust. It is
entirely optional: a game with no export simply has `GameProfile.frames ==
None`, silently (no warning, §7's "no silent caps" is about a run that
SKIPPED something, not about a port that never ran the lab at all).

The export currently carries TWO ROWS PER (char, move, variant, gap) cell,
one per observable (`docs/frames.md` §12) — on MK2 arcade always
`struct_velocity` and `pointer_x`. The two have agreed on every sweep
across two independent full runs (52 sweeps in the second alone), so the
chosen rule is: COLLAPSE agreeing observables into one row. A field where
the observables DISAGREE is the exceptional case (surfaced rather than
silently resolved — the collapsed value for that field is left `None` and
the field's name is recorded in `FrameCell::disagreements`, while each
observable's own raw value survives unedited in `FrameCell::observations`
for audit) — EXCEPT that "agree" means something different depending on
what kind of quantity the field is (`docs/frames.md` §8.4, corrected):

The class is NOT a property of a field's name — it is a property of HOW
THE ROW WAS MEASURED, specifically whether the value carries a probe
manifest's manifestation margin (`docs/frames.md` §8.4, corrected a second
time 2026-09-01):

- **Difference quantities** (`on_hit`, `on_block` — a manifest frame minus
  another manifest frame, both from the SAME observable) have their
  observable's own margin cancel out of the subtraction. Two observables
  must therefore agree EXACTLY. `hitstop` is also a difference quantity —
  connecting manifest minus whiffing manifest — for the same reason, and
  for the same reason stays exact even though it is a duration.
- **Anchor-based quantities** are bracketed by two reads of the SAME
  anchor signal (§4.1), never a behavioural probe, so neither endpoint
  carries a manifestation margin. `first_active_frame` (the contact signal
  relative to a fixed, software-controlled input frame) and `active` (the
  first and last contact-signal reads across a gap sweep) are both this
  shape. Held to exact agreement, like a difference quantity, but for a
  different reason (no margin ever entered the number, rather than two
  margins entering and cancelling).
- **One-sided quantities** carry a raw single-sided probe manifest's own
  manifestation margin `m` directly: `value = A_rel + m`, and — because
  §3.1 calibrates the OBSERVABLE, not the move — the very same `m` is
  baked into that observable's `input_latency_frames = l + m` (`l` the
  shared injection latency). So `value − input_latency_frames = A_rel − l`
  is invariant across observables measuring the same truth, independent of
  each one's own `m`. Two sound observables' raw values will therefore
  differ by exactly the difference in their `input_latency_frames` — NOT
  by zero. Mileena's roll: `wakeup_window` 77 (`struct_velocity`,
  latency 1) vs 78 (`pointer_x`, latency 2) is this agreement, not a
  disagreement (77 − 1 == 78 − 2 == 76). A one-sided field that does NOT
  satisfy this invariant is a REAL disagreement and is still flagged.
  `wakeup_window` (an anchor-to-actionable-manifest read for a knockdown)
  was the first field recognised as this shape. `total` and `recovery`
  are the SAME shape: under the only measurement protocol this project has
  (§4, the act-again probe), "recovered" has no anchor signal of its own —
  it is read the identical way `wakeup_window` is, from a fixed anchor to
  the actionable-again probe manifest, so it carries exactly one margin
  too. This was misclassified as anchor-based in the first cut of this
  rule (`docs/frames.md` §13 item 1): it cost nothing to notice on a
  contact-anchored move, because nothing has measured `total`/`recovery`
  there yet, but it blocked every WHIFF-anchored one outright — Reptile's
  invisibility has no contact to anchor on, so its `total` can ONLY come
  from the act-again probe, and reads 40 (`struct_velocity`, latency 1) /
  41 (`pointer_x`, latency 2) — agreement under this rule (40−1 == 41−2),
  disagreement under the old exact-match rule. Reclassifying is not
  loosening the check: it is applying the SAME rule already proven correct
  for `wakeup_window` to a field that is measured exactly the same way.

This is a true statement about the CURRENT protocol, not a permanent fact
about these field names — if a future observable can read "recovery ended"
directly off an anchor signal (no probe involved), that measurement would
be anchor-based instead. The schema has no way to say which shape a given
row is; today it is inferred from the field name because every row this
project has produced follows the current protocol. The correct fix is a
per-row column recording how the duration was bounded (e.g. `anchor_kind:
"dual_anchor" | "anchor_to_probe"`) so collapsing reads that off the row
instead of assuming it from the field's name — proposed in
`docs/frames.md` §12, not implemented here: nothing in this project's
scope (`shadow_train.framelab`, which would populate it) can be touched
from `src/profile.rs`, and a column nothing ever writes is not a real fix.

A one-sided field's collapsed value is the raw reading of whichever
observable in the cell has the SMALLEST `input_latency_frames` — by the
probe's own construction (`shadow_train.framelab.probe`) that observable's
margin `m` is zero, so its raw number already equals `A_rel` with nothing
to correct. `FrameCell::one_sided_reference` records, per such field, which
observable's frame of reference the collapsed number is in — a bare "77"
means different things in different rows, and that ambiguity is exactly
what this map exists to close off.
