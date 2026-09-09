# Placement lint — changelog-shaped detail in source comments (phase 0)

- corpus: `.` — 54 source file(s), 7513 comment line(s), 2726 block(s), **15 flagged** (conservation: every block classified or passed, none dropped)
- method: pure code — named lexical fingerprints over comment blocks (SATD-mining lineage). No model. Evidence shown verbatim; a dated comment is NOT always misplaced (date-parsing docs will match) — **the operator reads the evidence, not the flag.**
- destinations found in this project: DEBUGGING/notes doc → `DEBUGGING.md` · explanation doc → `ARCHITECTURE.md`
- action: RELOCATION IS YOURS. Nothing here edits anything; the VCS keeps the history either way.

## history (10) — proposed home: CHANGELOG (none exists yet)

_version-migration narrative — the VCS is the file history; keep-a-changelog for the curated story_

- `src/training.rs:1-37` (37 line(s); contrast, history-weak)
    - L27 [contrast] `//!   **finish round now** (F4, needs `round_state`) one-shots.`
    - L11 [history-weak] `//!   ([`crate::profile::TimerHold`]): the legacy `[sec, subsec]` pair`
- `src/training.rs:702-712` (11 line(s); contrast, history-weak)
    - L712 [contrast] `// harmless (the dummy blocks that one instead of punishing).`
    - L711 [history-weak] `// the previously documented "one spurious punish per refill", and`
- `src/shadow_runner.rs:2321-2330` (10 line(s); contrast, history-weak)
    - L2328 [contrast] `/// to asurabld by other tests in this binary) — `RunnerAddrs` now owns`
    - L2323 [history-weak] `/// the parts irrelevant to this test) — used to prove [`read_fighter`]`
- `src/record.rs:1686-1694` (9 line(s); contrast, history-weak)
    - L1691 [contrast] `/// damage delta). This USED to run on mk2's HUD-pair fallback; mk2 now`
    - L1691 [history-weak] `/// damage delta). This USED to run on mk2's HUD-pair fallback; mk2 now`
- `src/debug/mod.rs:582-589` (8 line(s); contrast, history-weak)
    - L586 [contrast] `/// read it) so a silent dummy explains itself instead of looking broken`
    - L587 [history-weak] `/// — an abort is exactly the case that USED to freeze on a stale`
- `src/debug/panels/training.rs:395-398` (4 line(s); contrast, history-weak)
    - L398 [contrast] `/// be a single hardcoded constant; this is that same knob, now a setting.`
    - L397 [history-weak] `/// macro starts relative to its trigger. `training::PUNISH_DELAY` used to`
- `src/training.rs:46-49` (4 line(s); contrast, history-weak)
    - L48 [contrast] `/// globals are the same pairing). Deriving it from live X instead — as this`
    - L49 [history-weak] `/// used to — mis-attributes the dummy the moment the fighters cross up.`
- `src/debug/panels/controls.rs:194-195` (2 line(s); contrast, history-weak)
    - L194 [contrast] `/// Controls that previously emitted this action's chord and were removed`
    - L194 [history-weak] `/// Controls that previously emitted this action's chord and were removed`
- `src/record.rs:1088-1090` (3 line(s); version-delta)
    - L1089 [version-delta] `// 0, so `health_in_range` fails and the gate must be CLOSED (v1's gate`
- `src/input_config.rs:446-446` (1 line(s); version-delta)
    - L446 [version-delta] `// v1 files wrote keyboard values as bare buttons; v2 is chord lists.`

## dated-log (5) — proposed home: `DEBUGGING.md`

_dated session/measurement notes — a lab journal entry, not an invariant of the code_

- `src/profile.rs:577-666` (90 line(s); contrast, iso-date, measurement-log)
    - L586 [contrast] `// The export currently carries TWO ROWS PER (char, move, variant, gap) cell,`
    - L601 [iso-date] `// time 2026-09-01):`
    - L599 [measurement-log] `// THE ROW WAS MEASURED, specifically whether the value carries a probe`
- `src/training.rs:1209-1219` (11 line(s); iso-date, measurement-log)
    - L1213 [iso-date] `/// (2026-09-01, port 4030): Block re-held at every frame from press+1 to`
    - L1213 [measurement-log] `/// (2026-09-01, port 4030): Block re-held at every frame from press+1 to`
- `src/training.rs:1151-1160` (10 line(s); iso-date, measurement-log)
    - L1154 [iso-date] `/// (live-observed on MK2 arcade, 2026-08-28). This is`
    - L1154 [measurement-log] `/// (live-observed on MK2 arcade, 2026-08-28). This is`
- `src/training.rs:1198-1206` (9 line(s); iso-date, measurement-log)
    - L1201 [iso-date] `/// 2026-09-01, port 4030: release-gap 7 fails at every guard-hold length`
    - L1200 [measurement-log] `/// input-eat OUTLIVES the Block release by ~8 frames (live-measured`
- `src/profile.rs:411-417` (7 line(s); iso-date)
    - L417 [iso-date] `/// 2026-09-02).`

