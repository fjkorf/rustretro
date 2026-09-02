---
name: encode-moves
description: Audit a character's special-move inputs live — drive both readings, require the move to actually come out, and record the disproven ones
---

# encode-moves — live audit of special-move encodings

Contract: `shadow/MACRO_ACTIONS.md` (the DSL, the matcher/executor pair, and
the timing bounds). Encodings live in the port profile's `special_inputs`;
the move VOCABULARY lives in `family.json`'s `moves`.

**Published movelists are hypotheses.** This protocol has overturned four of
them plus one of our own entries marked VERIFIED — a wrong charge duration
(off by 5x), a disproven published input, a move that does not swap sides
where the contract assumed it did, and a "verified" special whose evidence
turned out to be a normal's damage.

## The rule that makes an audit worth anything

**A move is verified when it PRODUCES ITS MOVE, at a range where nothing
else can.** Not when the input looks right, not when the screen looks right.

- Throw from a gap **past the whiff edge of the entire measured normal
  connect map**, so any damage is necessarily the special. Point blank is
  where a normal's damage gets credited to a special — that exact failure has
  now appeared three ways (a chorded encoding that never fires; a projectile
  measured inside proximity range; a mid-walk arena leaving a direction
  LATCHED so the first tap is not a fresh onset).
- Carry **negative controls in every batch**: no input, bare button, wrong
  button, wrong direction, one tap. All must produce zero.
- **Never audit at a move's reach boundary.** Connection there depends on the
  defender's idle-animation phase, and that reference DRIFTS across a
  session — a batch that verified a move stopped reproducing an hour later
  from the same save state with byte-identical positions.

## Both readings, always

Where a published notation is ambiguous, **drive both candidates and let the
game decide.** Do not reason about which is likelier; two identically-written
inputs have resolved differently in the same game.

Bisect every threshold **from both ends** and report the boundary pair
(`N fails 3/3, N+1 fires 3/3`). Durations, charge times, inter-step gaps and
proximity limits are all measurable this way, and all have been wrong when
transcribed.

## Observables for "it came out"

- **Damage** at a no-normal range is the cleanest.
- **Position/`y`** discriminates a teleport, a roll, a knockdown.
- **The framebuffer** is the last resort and it works: displace the fighter
  and count moved pixels — if he is drawn, walking moves pixels; if not,
  nothing. That is how a no-damage invisibility was verified.
- An action counter that fires on ENTERING an action cannot tell "came out
  and whiffed" from "did not come out". Only a signature can.

## What to write down

- The verified encoding into `special_inputs`, the move name into
  `family.json`'s `moves`.
- **The DISPROVEN readings**, into `library/<family>/<port>.md`, house style.
  A disproven reading not recorded is a reading someone re-derives.
- Any DSL feature the kit needs that the schema lacks, with the measurement
  that justifies it — timing bounds and preconditions have all arrived this
  way, one character at a time.
- An honest UNVERIFIED where no reliable observable exists. A fabricated pass
  is worse than a stated gap.

## Before you start

Read `/re-probe` for transport, `/frame-lab-run` for the measurement laws
that also apply here — especially "identify a move by its measured
signature" and "the probe can cancel the move it measures". Verify the rig
(both ports live, char ids read from the fighter blocks) before trusting any
result; building a rig for a character nobody has used yet is often the
larger half of the task, so cache it and give it one owner.
