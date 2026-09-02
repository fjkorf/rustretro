---
name: frame-lab-run
description: Measure frame data (startup / on-hit / on-block / hitstop / active / wakeup) for a character with the act-again probe, its preconditions, and its refusal rules
---

# frame-lab-run — measuring frame data

Contract: `docs/frames.md` (normative — read it, it is corrected often).
Harness: `shadow_train.framelab` (`session` owns preconditions, `probe` the
act-again measurement, `kit` the per-character runner, `arenas`/`spacing` the
ladder, `guard`/`punish`/`hitstop`/`active`/`airborne`/`specials`/`cancels`
the specialised passes). Authoring store `shadow/framelab/frames.db`
(gitignored); the COMMITTED artifact is `library/<family>/<port>.frames.json`.

**This skill carries the PROTOCOL. It carries no game's numbers.** Anchors,
observables, calibrations, floors and rig directions live in the port
profile's `framelab` block; what is known lives in `library/<family>/<port>.md`.
An uncalibrated port DECLINES by name — that is correct, not a bug to route
around. Putting one game's fact in standing guidance is the cross-game-fact
error institutionalised, and it has already happened once in an agent brief.

## Launch

See `/re-probe` for the launch command, the MCP client, and the transport
quirks — do not duplicate them here. Frame-lab additions:

- **`--pace 0`** always. Emulation at wall-clock pace was ~40x the cost of
  everything else; uncapped is ~1416 fps on MK2 arcade.
- **`step` is SYNCHRONOUS** (returns when the frame is complete) and
  **`run_frames(count, port0?, port1?)`** batches up to 600. Use them.
- **Never port 4025** — the user's. Agents take 4030+, one port each; two
  agents sharing a port or an arena filename is a collision waiting to happen.
- **Write run artifacts into the repo or the job tmp dir, never `/tmp`** — a
  sandbox that cannot `stat` its own polling path burns hours after the work
  has finished.

## Preconditions (`docs/frames.md` §3) — a run that skips one is void

1. Training enforcement OFF and the shadow runner off — refill rewrites the
   bytes an anchor reads.
2. `hold_buttons`/`release_buttons` ONLY. **`press_buttons` is BANNED.**
3. Confirm the input FOLD before input-sensitive work; `executed_*` reports
   what a frame that already ran actually saw.
4. `load_state(pause_after=True)`. **Never bracket a load with
   `resume`/`pause`** — plain `pause` is still fire-and-forget.
5. Arena liveness re-verified after every load, with a probe window long
   enough for THIS character.
6. Calibration current for this core build and ROM.
7. **No sleeps as fixes.** A settle that "fixed" a flake measured slightly
   worse than nothing; the real bug was a lock ordering race.

## The laws (each one was paid for with a wrong number)

1. **No ABSOLUTE observation of motion means anything.** Run the identical
   scenario with and without the input and believe only the difference. This
   killed three designs: the first probe draft, the arena liveness probe
   (which called a CPU-driven port live), and a contact-signal claim.
2. **Cross-method agreement buys PRECISION, never TRUTH.** Three wrong
   numbers survived because two observables agreed — they shared a flawed
   subtraction, a stale input frame, and incompatible units. What confirms a
   number is a **second RIG with a different readout**, not a second
   observable. The one number ever *confirmed* rather than corrected was
   hitstop, when an unrelated input-timing rig reproduced it exactly.
3. **Never anchor on a DRAWN value.** A displayed bar that animates toward
   its target reports a smear of edges where the event had one.
4. **Absent is never 0**, and a cap that renders like a measurement is the
   most dangerous kind — a whiff glyph hid an entire move twice. Every
   unmeasured cell needs a REASON.
5. **Calibrate the probe's OWN input shape, at TWO points, and require
   agreement.** A different probe shape has a different latency; a single
   point near contact reads residual stun as latency. The second point is
   DERIVED from the observed window, never a constant.
6. **The probe can CANCEL the move it measures** and report success doing
   it. Scan for the frames that kill a move; refuse a boundary landing on one.
7. **Identify a move by its measured SIGNATURE**, not by the buttons pressed.
   "These normals never reach" was clean, plausible and false; a special's
   name has three times been attached to a normal's damage.
8. **A default correct for two subjects is not a law.** Every scaling bug so
   far was a default fitted to earlier characters — contact horizon, stance
   lead-in, liveness window, settle frames. Measure them per subject.
9. **Some hazards are invisible to BOTH safeguards.** Air control during a
   jump: differencing does not protect you (the control is not drifting) and
   cross-observable agreement does not either (both observables move
   together). Those need a dedicated experiment, and one had never been run.
10. **Verify VALUES, not counts.** The store and the export diverged with row
    counts matching while a whole column was stale.

## Protocol shapes

- **Act-again probe** (§4): differential replay — probe run vs identical
  no-input control; `actionable(N) := observable differs`. Linear sweep is
  the DEFAULT; binary search is opt-in and only where monotonicity has been
  demonstrated. Advantage is the difference of raw MANIFEST frames — do NOT
  subtract per-side calibration when the two sides have different probe
  shapes.
- **Hitstop**: whiff-differenced attacker manifest — the same script
  connecting minus whiffing. A freeze detector is BLIND where a struct is
  silent through the whole stun window.
- **Active frames**: teleport a body into the hitbox at frame N and sweep N;
  the contact window is the active window. Validate its start against
  `first_active_frame`, which measures the same edge from the other side.
- **Airborne**: the probe is not blind mid-flight, it is DEFERRED to after
  landing. Run an air-control scan first, and derive the calibration point
  from that run's own landing.
- **Replay-sourced**: a slot is valid only against the state it was recorded
  from. Anchor on OBSERVED contact, never the expected one; classify
  ON-TIME / RETIMED / WHIFF / NO-EXECUTE / DIVERGED and refuse the last two.

## Exit criteria

Rows carry method, observable, calibration, `sample_n`, `core_id`, `rom_id`;
NULL never 0. **Re-export and verify field-by-field against the store.** At
least one cell cross-checked; better, one cell confirmed by a different rig.
Evidence to `library/<family>/<port>.md` in house style — method, table,
confidence per row, and **everything skipped, named**. Refusals are results:
record them. Kill your instance.
