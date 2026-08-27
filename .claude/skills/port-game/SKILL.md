---
name: port-game
description: Bring a new game (or a new port of an existing family) into the pipeline — the tier ladder with exit criteria, from first boot to shadow-ready
---

# port-game — the tier ladder

Per-game knowledge is DATA (`library/<family>/family.json` + `<port>.profile.json`,
schema in docs/game-profiles.md). Addresses never go into code. Climb in order; each
tier has an exit criterion. Feature consumers degrade per-feature, so partial tiers
are useful — never fake completeness.

## T0 — boots
Pick the core (house rule: from-source FBNeo first when a driver exists — source access
is an instrument; `grep` the driver in ../FBNeo). Verify: renders, RAM regions exposed
(`list_regions`), save states round-trip, input descriptors, `memory.cpu` correct
(Sek capture on a non-68k SEGFAULTS — gate it). Romset naming traps: FBNeo console
drivers select on the prefixed name but search for the stripped name — isolate in a
subdir with both. **Exit: headless boot + save/load + a written `requires` block.**

## T1 — gated
Find the in-fight gate, expressible ONLY in the closed vocabulary (byte_zero /
word_zero / health_in_range / bcd_valid_nonzero). Verify against EVERY phase: title,
menus, char select, round intro, live fight, finish/victory, demo, and IN-GAME PAUSE
(pause is a known gate-invisibility trap — hunt the pause flag early). Candidates from
external sources enter as unverified-with-provenance; live-verify before the profile.
Beware: cheat tables lie across romsets/revisions; community maps can be for a
different platform entirely. **Exit: `game.controllable()` correct in all phases;
in-fight arena state captured as the conformance fixture.**

## T2 — recordable
Fighter blocks/stride + at minimum char_id, health, x (the feature contract requires
x/char_id/health). Verify char ids against the roster by picking characters; ids are
canonical family-wide (add `id_map` only if a port's raw ids diverge). Set calibration
from OBSERVED play (GROUND_Y = the real standing y across a session, not a one-off
snapshot; omit CORNER_PX/SCREEN_W if x is world-position). **Exit: a recording whose
rows carry exactly the mapped fields, and a fit that produces sane bucket_counts.**

## T3 — enforceable
Per-feature: refill (all health accumulators — games keep independent copies), timer
hold (write-test the store: a "revert" may be your value ticking), position reset
(explicit positions only), dummy (block style from family.json). Settings the game
keeps in volatile RAM (pad mode, difficulty) become profile `pins`. **Exit: training
panel shows the honest feature list; refill visibly fires in play.**

## T4 — shadow-ready
Attack chords verified per-button IN THE RIGHT PAD MODE (console games may need their
own 6-button option — pin it), full loop: record real play → fit → playback vs the
ghost. **Exit: a human fought the model.**

## Always
- Evidence doc (`library/<family>/<port>.md`) grows with every claim: method, value,
  date, and DISPROVEN candidates/traps. Consolidate strata into topical sections at
  phase end — chronology is for sessions, topics are for readers.
- Use `/re-probe` for the live-session protocols.
- Data roots are per-FAMILY (`shadow/{models,recordings,arenas}/<family>/`) — ports
  share them by design (cross-port experiments).
