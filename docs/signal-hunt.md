# Signal hunt — event-marked differential RAM analysis

Normative contract for the in-app RE feature. It exists because the same
manual protocol was run five times in one session (MK2 hit counter, MK2
action counter, the MK2 pause flag, asurabld contact, asurabld `attacking`),
each costing an agent-hour of bespoke scripting, and every new game needs it
on day one.

## 1. The protocol being automated

1. Get into the phase where the event happens.
2. Snapshot a RAM region continuously.
3. Mark the moments an event occurs ("a blocked hit just landed").
4. Mark control moments where it did NOT ("that one whiffed").
5. Intersect what changed across event marks; subtract what changed across
   control marks and during idle; rank what survives.

Steps 2–5 are the app's job. Steps 1, 3, 4 are the human's (or an agent's) —
the judgement of "that was a blocked hit" is exactly what cannot be
automated, and pretending otherwise is how false signals get shipped.

## 2. Marking

- `hunt_mark(label)` — MCP tool, Lua binding, and a debugger hotkey.
  `label` is free-form; by convention `event` labels are what you are hunting
  and `control` labels are near-misses. Multiple labels may be in play.
- A mark records: frame number, wall-clock, label, and the gate state at that
  frame (so marks taken while not in a fight are visibly suspect).
- Marks are cheap; over-marking is fine. `hunt_reset` clears them.

## 3. The ring buffer

- Continuously retain the last `HUNT_RING_FRAMES` (default 60) snapshots of
  the **hunt region**.
- The hunt region MUST be scoped, not "all of RAM": MK2 arcade's exposed
  region is 2.3 MB, which at 60 frames is 138 MB. Default scope =
  the two fighter structs (block1/block2 + stride from the profile) plus an
  optional extra window; configurable in the panel and via
  `hunt_configure(start, len)`.
- Sampling MUST be cheap enough not to disturb frame pacing. If the region
  exceeds a sane budget, refuse with a message naming the size rather than
  silently degrading — a silently-truncated hunt produces confident wrong
  answers, which is worse than no hunt.

## 4. Analysis

`hunt_analyze(event_label, control_label?)` returns ranked candidates.

For each mark of `event_label`, the changed-set is the byte offsets differing
between the snapshot at `mark - PRE` and `mark + POST` (defaults 4 and 12
frames; configurable — a reaction may take a few frames to appear).

- **Event set** = intersection of every event mark's changed-set. A byte that
  misses even one event is not the signal.
- **Control set** = union of every control mark's changed-set, PLUS the idle
  churn set (bytes that differ between consecutive quiet snapshots with no
  mark nearby). Union, not intersection: any byte that ever moves without the
  event is disqualified.
- **Candidates** = event set − control set.

Rank by: fires on all events (required), then prefers small values (< 16),
then counter-like transitions (monotone or fixed-delta across marks), then
byte-over-word. Report the per-mark value transitions for every candidate —
the reader must be able to judge, not just trust the ranking.

## 5. Output

- Addresses render PROFILE-RELATIVE where possible (`block2+0x6F`, not a bare
  address), because that is the form that goes into a profile.
- An export action emits the evidence-doc format used by
  `library/<family>/<port>.md`: method, marks used, candidates with
  transitions, and an explicit "controls used" line.
- **The tool never writes a profile.** Candidates are hypotheses; promotion to
  a profile requires a write-test, which is a human/agent decision. The export
  text says so.

## 6. Honesty requirements (each one is a bug we shipped or nearly shipped)

- If zero candidates survive, say so plainly. "No byte fires on every event
  and stays quiet on the controls" is a RESULT — it is what the Genesis
  contact-signal hunt correctly concluded.
- If no control label was supplied, the report must warn prominently. A hunt
  without a control is how `action_counter` was briefly mistaken for a contact
  signal.
- Marks taken while the profile gate was closed are flagged in the report.
- The report states the ring/PRE/POST settings used, since a candidate that
  only appears at one window setting is suspect.

## 7. Acceptance — rediscovery of known answers

The feature is done when it reproduces findings we already made by hand:

- **asurabld**: ~5 marks on blocked contact + ~3 whiff controls must surface
  `block+0x6F` (`attacking`) among the top candidates, with no bespoke
  scripting.
- **MK2 arcade**: blocked-contact marks + whiff controls must surface the HUD
  health pair and must NOT rank the action counter (`+0xC0`) as a contact
  signal — it fires on the attacker's swings, and the control set should
  eliminate it. This is the regression test for the specific mistake made on
  2026-08-28.

## 8. Surfaces

- Debugger panel 🔍 Signal Hunt: mark buttons (event/control, editable label),
  live mark counts, Analyze, a candidate table, Export, Reset.
- MCP: `hunt_mark`, `hunt_analyze`, `hunt_configure`, `hunt_reset` — so agents
  can run the whole protocol headlessly.
- Lua: `hunt.mark(label)` for scripted marking from a per-frame callback.
