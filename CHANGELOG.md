# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Versions are not tagged yet; the sections below are curated history relocated
from source comments by the loom placement lint, each with its source provenance.

## Relocated from src/training.rs:702-712 (loom lint F-03, ratified 2026-09-09)

`direction: "decrease"` (contact_signal): only a DROP counts as
contact. This is what makes a health-valued signal (MK2's struct
health) immune to both INCREASE hazards by one sign check — the
round-intro ramp (+2/frame under the banner-gate leak) and the
training refill writing health back to max. Increases also don't
stamp the quiet-window bookkeeping, so a refill can't hold the
cooldown open. ACCEPTED LOSS: the hit that drives health below the
refill threshold is overwritten back to max by refill before the next
poll, so ~one real trigger per refill cycle is lost — the inverse of
the previously documented "one spurious punish per refill", and
harmless (the dummy blocks that one instead of punishing).

## Relocated from src/training.rs:46-49 (loom lint F-11, ratified 2026-09-09)

The dummy occupies fighter block 2: it is injected on controller port 1,
and port 1 drives block 2 (asurabld.md verified this live; MK2's `p2_*`
globals are the same pairing). Deriving it from live X instead — as this
used to — mis-attributes the dummy the moment the fighters cross up.

## Relocated from src/record.rs:1686-1694 (loom lint F-07, ratified 2026-09-09)

MACRO_ACTIONS §8 item 2: a contact event with NO health change on the
defender classifies as `no_damage`, never asserted as "blocked" — it
still opens/holds a string, and a string with zero hits counts as a
block string. Exercises the asurabld shape (hitstun_sources over the
combo counters, distinct from the `health` fighter field used for the
damage delta). This USED to run on mk2's HUD-pair fallback; mk2 now
ships `contact_signal` field=health direction=decrease, whose every
event carries damage by construction (blocked contact always chips
there) — see the mk2-specific test below.

## Relocated from src/record.rs:1088-1090 (loom lint F-14, ratified 2026-09-09)

A bare DebugState has no regions, so all reads return 0: healths are
0, so `health_in_range` fails and the gate must be CLOSED (v1's gate
was true here — the broken-permissive bug the v2 rewrite fixed).

## Relocated from src/debug/mod.rs:582-589 (loom lint F-09, ratified 2026-09-09)

Human-readable BlockPunish phase, refreshed every frame the mode
runs: "guarding — armed" / "cooling (Nf)" / "punishing: slide" /
"aborted — <reason>" / "unavailable …". The ONE place this is
computed (panel, Lua `training.punish_state()`, and any overlay all
read it) so a silent dummy explains itself instead of looking broken
— an abort is exactly the case that USED to freeze on a stale
"punishing: slide" while the gate was closed (misdiagnosed live);
this field must say "aborted" instead (MACRO_ACTIONS §10.1).
