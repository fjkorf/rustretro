"""shadow_train.framelab.harvest — free, noisy advantage observations mined
from ordinary recordings, offline (docs/frames.md, task G2).

**This module never writes to the frame store.** `framelab.store.FrameStore`
is a MEASUREMENTS store (docs/frames.md §6): rows in it are produced by the
calibrated act-again probe (`framelab.probe`), with a control run, a named
observable, and a recorded `input_latency_frames`. Nothing this module
produces has any of that. A harvested number is an OBSERVATION, not a
measurement, and it is labelled that way everywhere it is surfaced —
`Observation`/`CandidateCell`/`AuditFinding` below carry no `insert()`-shaped
schema and this module imports nothing from `framelab.store`.

## What a recording lets us compute for free

Every jsonl-v2/v3 recording already carries, every frame, both fighters'
health and both physical ports' raw input masks (RECORDER_V3.md §1.2). That
is enough to find a CONTACT (a defender's mapped `health` field dropping
frame-to-frame — the same struct-health anchor docs/frames.md §4.1 names,
never a HUD/drawn value) and to ask, from the same single recorded
playthrough: **which side pressed something new first, and how many frames
after the other one?**

That is not the lab's `act-again` probe. The probe compares an identical
scripted replay WITH an input against one WITHOUT it, and calls the fighter
actionable at the first frame those two diverge (docs/frames.md §4.2) — it
can therefore prove a fighter COULD NOT act before frame N. A harvested
observation has no control run: it can only report that a human COULD act
by the frame they were seen doing something recognizably new. It is
therefore a fundamentally noisier quantity, and it is noisy in BOTH
directions at once, which is why this module never claims to bound the true
advantage on one side only:

- a human does not act on the first possible frame (reaction time), which
  pushes an observed "next action" LATER than the true actionable frame;
- a human can also BUFFER an input during hitstun/blockstun before they are
  actually free, which can make an observed "next action" register EARLIER
  than the true actionable frame.

Both effects are live in every observation this module produces. What
survives that noise is the comparison the task asks for: if the measured
table says a move cannot be worse than some bound and a harvested
observation is MORE extreme than that bound, at least one of "the
observation's heuristic mis-fired" and "the measured row is wrong" is true,
and the pairing is worth a human's attention (`audit_against_table` below).
An observation that merely agrees with the table proves nothing — agreement
is the expected, uninteresting case.

## The "acted again" heuristic (be explicit, because it is the whole trick)

For one side (attacker or defender) at a contact anchored at frame C:

1. Take the OR of that side's own raw input mask (`p1_input`/`p2_input`,
   whichever physical port drives that block — RECORDER_V3.md §1.2 rule 5)
   over frames `[C - BASELINE_WINDOW + 1, C]`. Call this the BASELINE —
   "whatever they were already holding going into the hit."
2. Walk forward from `C + 1`. The first frame whose mask has a bit NOT in
   the baseline is that side's observed "acted again" frame. A continued
   hold of the same direction is not a new action; a fresh press (of
   anything — a direction, an attack button) is.
3. If no such frame appears within `NEXT_ACTION_MAX_WAIT` frames, that side
   is UNOBSERVED for this contact and the contact contributes no advantage
   number (counted, never silently dropped — `contacts_skipped_no_action_*`).

`observed_advantage = defender_next_action_frame - attacker_next_action_frame`
— the same "difference the raw manifest frames" convention docs/frames.md
§4.3 uses for the lab's own two-probe difference, applied to two single-shot
observations instead of two controlled probes. Positive means the attacker
was seen acting again first (the move reads as safe-ish); negative means the
defender was seen first (the move reads as unsafe-ish) — same sign
convention as the exported `on_hit`/`on_block` columns.

## Attribution — special vs normal vs UNATTRIBUTED

A wrongly attributed row is worse than an unattributed one (task spec), so
attribution only ever fires when it is unambiguous:

1. **Specials**: the recorder's live macro matcher already annotates
   `p1_special`/`p2_special` on the completion frame (`src/record.rs`,
   `shadow/MACRO_ACTIONS.md` §3/§4). This module reads that annotation
   directly (it is exactly the signal the task names) rather than
   re-deriving it — asurabld ships no `moves`/`special_inputs` table at all
   (`dataset.py`'s `reload_profile` comment), so re-deriving would be a
   silent no-op there anyway, while mk2 arcade's table is real and the
   annotation is authoritative for it.
2. **Normals**: the profile's own `attack_chords` (class name -> button
   list) is turned into a bit-universe of every button any class uses, and
   a class is attributed ONLY when the attacker's newly-pressed bits within
   that universe exactly equal one class's own bit-set (mirrors
   `dataset._attack_class`'s exact-popcount resolution, generalised to
   named per-class bit-sets so it also covers mk2's four independent
   single-button classes). Two classes sharing an ambiguous bit pattern, or
   a combination matching no class at all, is UNATTRIBUTED — never a guess.
   `down` held at that onset prefixes a `c` (`cHP`), matching the exported
   table's own crouch-normal naming; this is a NAMING convention only, not
   a verified move identity (docs/frames.md §4.3's "a move must be
   identified by its measured SIGNATURE" is a lab requirement this
   input-only heuristic cannot meet — attributions here are for candidate
   ranking and loose auditing, not a claim that the intended move actually
   connected).

## Position is NOT load-bearing here, and mk2 arcade's is untrusted anyway

`x` is used for exactly one thing: an optional `gap_px` on the observation,
purely descriptive (never required for the advantage number or for
attribution — neither uses facing or position). Per RECORDER_V3.md §1.2
rule 1 / docs/frames.md §2.5, a POINTER-RESOLVED `x` field can be legally
absent on individual rows; when the sidecar's `pointer_resolved_fields`
names `x` and it is missing on the frames a contact needs, this module
skips that contact ENTIRELY (not just the gap) and counts it
(`contacts_skipped_pointer_unresolved_x`) — emitting the rest of the
observation with `gap_px=None` would be indistinguishable from "this port
has no position data at all," which is a different, worse-to-hide fact.

Separately, and worth stating plainly for anyone reading harvested `gap_px`
numbers: the mk2 arcade recordings on this machine source `x` through the
`p1_x`/`p2_x` GLOBALS (RECORDER_V3.md §2.5's global-sourced-field escape
hatch), which is exactly the position source docs/frames.md §4.2 rule 3
calls FORBIDDEN and `library/mk2/mk2.md` disproves (a stale object-pool
slot that reads frozen through visually-confirmed movement) — the fix
(the `block-0xC` object pointer) landed in the profile after these
recordings were made. `gap_px` from those files is reported but should not
be trusted; see `run_report`'s printed caveat.

## Honesty bookkeeping

`HarvestStats` counts every reason a file, round, or contact produced no
observation — v1 recordings (rejected, per RECORDER_V3.md §1.1), a profile
mapping no `health` field, a pointer-unresolved `x`, a side never observed
acting again. Nothing here silently caps or drops without a counter. A run
that finds zero usable contacts reports that as `observations_usable == 0`
rather than raising or fabricating rows — see `format_report`.
"""

from __future__ import annotations

import json
from collections import Counter, defaultdict
from dataclasses import dataclass, field
from pathlib import Path
from typing import Optional

from .. import macros as _macros
from ..dataset import _detect_version, _view_for, fields

# This module imports nothing from `.store` (the SQLite authoring store) --
# see the module docstring's first paragraph. There is deliberately no path
# from anything below to `FrameStore.insert()`.

__all__ = [
    "QUIET_FRAMES",
    "ATTACK_LOOKBACK_FRAMES",
    "NEXT_ACTION_MAX_WAIT",
    "BASELINE_WINDOW",
    "AUDIT_TOLERANCE_FRAMES",
    "Observation",
    "HarvestStats",
    "CandidateCell",
    "AuditFinding",
    "harvest_file",
    "harvest_files",
    "rank_candidates",
    "load_measured_table",
    "audit_against_table",
    "format_report",
]

# ── tunable constants (heuristic, documented, not measured — §7 requires
# every MEASURED number to carry provenance; these are not measurements) ──

# §4.1's "consecutive contacts inside the counter's ~20-frame window do not
# reset it" multi-hit clustering window. 20 matches both HITSTUN_RECENT_
# FRAMES (dataset.py) and mk2's own framelab.quiet_frames -- not a
# coincidence (both derive from the same combo-gap evidence), but this
# module hardcodes 20 as ITS OWN convention rather than reading either,
# because neither exists for every port (asurabld's framelab block is
# absent) and this module must run on any family/port pair a recording
# names.
QUIET_FRAMES = 20

# How far back from a contact frame to search for the attacker's causing
# input (a special annotation or an attack-chord onset). Generous relative
# to every measured FAF in library/mk2/arcade.frames.json (8-12 frames) so
# normal human play (which is not the lab's point-blank minimum-gap rig)
# still gets attributed; wide enough that it can occasionally catch an
# unrelated earlier press in a fast mash -- exactly why ambiguous or
# unmapped onsets are UNATTRIBUTED rather than a best guess.
ATTACK_LOOKBACK_FRAMES = 45

# How long to wait, after a contact, for a side to press something new
# before giving up and calling that side unobserved for this contact.
NEXT_ACTION_MAX_WAIT = 90

# OR-window (frames, including the contact frame) used to build the
# "already held going into the hit" baseline mask a post-contact frame must
# diverge from to count as a new action.
BASELINE_WINDOW = 4

# Default slack (frames) before an observed advantage more extreme than the
# measured table's worst case counts as a contradiction worth flagging.
# Kept small and separate from the lab's own calibration numbers -- this is
# purely "how much heuristic slop do we tolerate before we bother a human,"
# not a measured quantity.
AUDIT_TOLERANCE_FRAMES = 3

_BLOCKS = ("block1", "block2")


def _other_block(block: str) -> str:
    return "block2" if block == "block1" else "block1"


def _port_for_block(block: str, p1_block_value: int) -> str:
    """Which physical RETRO port (`p1`/`p2`, i.e. which of `p1_input`/
    `p2_input`) drives `block` this round, from the round's own resolved
    `p1_block` anchor (RECORDER_V3.md §1.2 rule 4)."""
    is_p1 = (block == "block1" and p1_block_value == 1) or (
        block == "block2" and p1_block_value == 2
    )
    return "p1" if is_p1 else "p2"


@dataclass
class Observation:
    """One harvested, noisy, lower-bound-flavoured advantage observation.

    Every field here is either read straight off the recording or derived
    by the heuristics documented at the top of this module -- nothing here
    is a `framelab.store` row and nothing here should ever be inserted into
    one (no method to do so is provided)."""

    file: str
    round_id: int
    frame: int                       # anchor frame (last contact in the group)
    hits: int
    family: str
    port: str
    attacker_char: Optional[str]
    defender_char: Optional[str]
    move: Optional[str]              # attributed class name, or None
    move_source: str                 # "special" | "chord" | "unattributed"
    observed_advantage: Optional[int]
    attacker_next_frame: Optional[int]
    defender_next_frame: Optional[int]
    gap_px: Optional[float]


@dataclass
class HarvestStats:
    """Every reason a file/round/contact produced fewer observations than
    contacts found -- §7's "no silent caps" as counters, not prose."""

    files_total: int = 0
    files_skipped_unsupported_version: int = 0
    files_skipped_no_health_field: int = 0
    files_skipped_error: int = 0
    rounds_seen: int = 0
    contacts_found: int = 0
    contacts_skipped_pointer_unresolved_x: int = 0
    contacts_skipped_no_action_attacker: int = 0
    contacts_skipped_no_action_defender: int = 0
    observations_usable: int = 0
    observations_attributed: int = 0
    skipped_files: list = field(default_factory=list)   # [(path, reason)]

    def add(self, other: "HarvestStats") -> None:
        for f in (
            "files_total", "files_skipped_unsupported_version",
            "files_skipped_no_health_field", "files_skipped_error",
            "rounds_seen", "contacts_found",
            "contacts_skipped_pointer_unresolved_x",
            "contacts_skipped_no_action_attacker",
            "contacts_skipped_no_action_defender",
            "observations_usable", "observations_attributed",
        ):
            setattr(self, f, getattr(self, f) + getattr(other, f))
        self.skipped_files.extend(other.skipped_files)


@dataclass
class CandidateCell:
    """A (family, port, attacker char, move) the lab has not necessarily
    measured yet, ranked by how often real play actually produced a contact
    attributable to it -- the "what to measure next" half of the task."""

    family: str
    port: str
    char: str
    move: str
    count: int
    sample_advantages: list


@dataclass
class AuditFinding:
    """An observation that CONTRADICTS the measured table -- more extreme
    than the table's own worst-case bound by more than `tolerance` frames.
    This is a candidate for re-measurement, never a verdict: see the module
    docstring's "noisy in both directions" section for why a single
    contradicting observation does not, by itself, prove the table wrong."""

    family: str
    port: str
    char: str
    move: str
    observed_advantage: int
    measured_bound: int
    tolerance: int
    file: str
    round_id: int
    frame: int
    detail: str


def _rounds_for_harvest(path: Path):
    """Yield (round_id, rows) for controllable, non-demo rounds -- same
    controllable/anchor/demo filter as `dataset._rounds`, deliberately
    reimplemented rather than imported: that function's minimum-length
    filter (`len(rows) < P * (K + 1)`) is a decision-cadence concern (P/K
    are the *currently loaded* profile's globals, mutated by
    `dataset.reload_profile()`), and this module must process files from
    several families/ports in one run without touching that shared,
    other-agent-owned global state. `_detect_version` is called for its
    v1-rejection side effect (raises `SystemExit`) only; row access below
    goes through `fields()`, which self-dispatches per row like the rest of
    `dataset.py` does."""
    _detect_version(path)
    rounds: dict = {}
    with open(path) as f:
        for line in f:
            try:
                r = json.loads(line)
            except json.JSONDecodeError:
                continue
            if not r.get("controllable") or r.get("p1_block") is None:
                continue
            rounds.setdefault(r["round_id"], []).append(r)
    for rid, rows in sorted(rounds.items()):
        if sum(x.get("p1_input", 0) for x in rows) == 0:
            continue  # attract/demo round (§5's demo filter)
        yield rid, rows


def _find_contacts(rows: list, view) -> list:
    """§4.1's anchor: a mapped `health` field dropping frame-to-frame, on
    either block. Consecutive drops within `QUIET_FRAMES` of the previous
    one are one contact group (multi-hit); the anchor frame is the LAST
    drop in the group, per §4.1's "anchor on the LAST contact before the
    quiet window, store hits" rule. Returns
    `[(anchor_idx, defender_block, attacker_block, hits), ...]`, in frame
    order across both blocks.

    Scanning starts at the round's first frame with ANY input on either
    port, not at row 0. `library/mk2/mk2.profile.json`'s own evidence notes
    "a known ~2s leak during the ROUND-N banner (inputs ignored there;
    recorder's zero-input filtering covers it)" -- and the real mk2 arcade
    recordings on this machine confirm it concretely: every round after the
    first opens with several dozen zero-input frames during which the
    mapped `health` field itself is seen RAMPING (e.g. 4, 6, 8, 10, ... up
    to 161, +2/frame) rather than holding steady -- a round-start
    health-bar-fill animation, not a hit (both blocks ramp together,
    symmetrically, with nobody having pressed anything yet). Treating that
    ramp's first tick as a "contact" would manufacture a huge, fictitious,
    zero-attribution drop on every round boundary. Zero input on both ports
    is exactly the signal the profile's own evidence already names for this
    leak, so this module reuses it rather than hardcoding a frame count."""
    if "health" not in view.field_names:
        return []

    start = 0
    for i, row in enumerate(rows):
        if row.get("p1_input") or row.get("p2_input"):
            start = i
            break
    else:
        start = len(rows)  # no input anywhere -- nothing to anchor on

    events = {"block1": [], "block2": []}
    prev_health = {"block1": None, "block2": None}
    for i, row in enumerate(rows[start:], start=start):
        for b in _BLOCKS:
            f = fields(row, b)
            if "health" not in f:
                continue
            h = f["health"]
            ph = prev_health[b]
            if ph is not None and h is not None and h < ph:
                events[b].append(i)
            if h is not None:
                prev_health[b] = h

    out = []
    for b in _BLOCKS:
        idxs = events[b]
        if not idxs:
            continue
        group = [idxs[0]]
        for idx in idxs[1:]:
            if idx - group[-1] <= QUIET_FRAMES:
                group.append(idx)
            else:
                out.append((group[-1], b, _other_block(b), len(group)))
                group = [idx]
        out.append((group[-1], b, _other_block(b), len(group)))
    out.sort(key=lambda t: t[0])
    return out


def _first_new_action(rows: list, anchor_idx: int, input_key: str) -> Optional[int]:
    """§ "the acted again heuristic": first frame after `anchor_idx` whose
    `input_key` mask carries a bit absent from the OR-baseline of the
    `BASELINE_WINDOW` frames ending at (and including) the anchor. `None`
    if nothing new appears within `NEXT_ACTION_MAX_WAIT` frames."""
    lo = max(0, anchor_idx - BASELINE_WINDOW + 1)
    baseline = 0
    for i in range(lo, anchor_idx + 1):
        baseline |= rows[i].get(input_key, 0)
    limit = min(len(rows), anchor_idx + 1 + NEXT_ACTION_MAX_WAIT)
    for i in range(anchor_idx + 1, limit):
        mask = rows[i].get(input_key, 0)
        if mask & ~baseline:
            return i
    return None


def _attack_bit_universe(attack_chords: dict):
    """Union of every button any DAMAGING `attack_chords` class uses, plus a
    bit-set -> class-name(s) reverse index, built from `macros._BUTTON_
    MASKS` (the one shared RETRO-button-name table, not re-declared here).

    `Block` is excluded: it is a reserved, non-attacking `attack_chords`
    entry that exists purely so macro chords can require it held/released
    (`shadow/MACRO_ACTIONS.md` §2 -- e.g. reptile's arcade `slide` needs
    `press: [LK, LP, Block]`) and both shipped mk2 port profiles carry one
    (`library/mk2/mk2.profile.json`'s `Block: ["l"]`,
    `genesis.profile.json`'s `Block: ["a"]`). Treating it like any other
    class was found, against the real recordings on this machine, to
    misattribute genesis contacts as caused by "Block" whenever the
    attacker's guard button happened to edge within the lookback window --
    exactly the false-attribution failure mode this module exists to avoid
    (a wrongly attributed row is worse than an unattributed one)."""
    universe = 0
    by_bits: dict = {}
    for cls, buttons in attack_chords.items():
        if cls.lower() == "block":
            continue
        m = 0
        for name in buttons:
            m |= _macros._BUTTON_MASKS[name]
        universe |= m
        by_bits.setdefault(m, []).append(cls)
    return universe, by_bits


def _attribute_move(rows: list, anchor_idx: int, attacker_port: str, prof):
    """§ "attribution" above. Returns `(move_name_or_None, source)` where
    `source` is `"special"`, `"chord"`, or `"unattributed"`. Never guesses:
    an ambiguous or unmapped chord onset is unattributed, not a best fit."""
    input_key = f"{attacker_port}_input"
    special_key = f"{attacker_port}_special"
    lo = max(0, anchor_idx - ATTACK_LOOKBACK_FRAMES)

    # 1) the recorder's own live macro-matcher annotation, nearest to the
    # anchor (RECORDER_V3.md §1.2's `p1_special`/`p2_special`).
    for i in range(anchor_idx, lo - 1, -1):
        name = rows[i].get(special_key)
        if name:
            return name, "special"

    # 2) an attack-chord button onset -- exact bit-set match only.
    universe, by_bits = _attack_bit_universe(prof.attack_chords)
    if universe:
        down_bit = _macros._BUTTON_MASKS["down"]
        prev = rows[lo].get(input_key, 0) & universe
        onset = None
        for i in range(lo + 1, anchor_idx + 1):
            cur = rows[i].get(input_key, 0) & universe
            if cur and not prev:
                onset = (i, cur)
            prev = cur
        if onset is not None:
            i, bits = onset
            classes = by_bits.get(bits)
            if classes and len(classes) == 1:
                cls = classes[0]
                down = bool(rows[i].get(input_key, 0) & down_bit)
                return (f"c{cls}" if down else cls), "chord"

    return None, "unattributed"


def harvest_file(path: Path, prof) -> tuple:
    """Harvest one recording file against `prof` (a loaded `GameProfile` for
    THIS file's family/port -- the caller resolves which one; see
    `harvest_files`). Returns `(observations, stats)`; never raises for a
    file this project already knows how to reject (v1, no health field) --
    those are counted in `stats` and yield an empty observation list."""
    path = Path(path)
    stats = HarvestStats(files_total=1)

    try:
        version = _detect_version(path)
    except SystemExit as exc:
        stats.files_skipped_unsupported_version = 1
        stats.skipped_files.append((str(path), str(exc)))
        return [], stats

    try:
        view = _view_for(path, version, prof)
    except Exception as exc:  # pragma: no cover - defensive, not expected
        stats.files_skipped_error = 1
        stats.skipped_files.append((str(path), f"view resolution failed: {exc}"))
        return [], stats

    if "health" not in view.field_names:
        stats.files_skipped_no_health_field = 1
        stats.skipped_files.append(
            (str(path), "profile maps no 'health' fighter field -- cannot anchor contacts")
        )
        return [], stats

    x_pointer_resolved = "x" in view.pointer_resolved_fields
    family = view.family or prof.family
    port = view.port or prof.port

    observations: list = []
    for rid, rows in _rounds_for_harvest(path):
        stats.rounds_seen += 1
        p1_block_value = rows[0]["p1_block"]

        for anchor_idx, defender_block, attacker_block, hits in _find_contacts(rows, view):
            stats.contacts_found += 1
            attacker_x = fields(rows[anchor_idx], attacker_block).get("x")
            defender_x = fields(rows[anchor_idx], defender_block).get("x")

            if x_pointer_resolved and (attacker_x is None or defender_x is None):
                # docs/frames.md §2.5 / RECORDER_V3.md §1.2 rule 1: an
                # absent pointer-resolved field this frame is a legitimate
                # per-row gap, not a zero. Emitting the rest of this
                # observation with gap_px=None would be indistinguishable
                # from "this port has no position data at all" -- skip the
                # whole contact and count it instead.
                stats.contacts_skipped_pointer_unresolved_x += 1
                continue

            gap_px = None
            if attacker_x is not None and defender_x is not None:
                gap_px = abs(defender_x - attacker_x)

            attacker_port = _port_for_block(attacker_block, p1_block_value)
            defender_port = _port_for_block(defender_block, p1_block_value)

            attacker_next = _first_new_action(rows, anchor_idx, f"{attacker_port}_input")
            if attacker_next is None:
                stats.contacts_skipped_no_action_attacker += 1
                continue
            defender_next = _first_new_action(rows, anchor_idx, f"{defender_port}_input")
            if defender_next is None:
                stats.contacts_skipped_no_action_defender += 1
                continue

            observed_advantage = defender_next - attacker_next
            stats.observations_usable += 1

            move, source = _attribute_move(rows, anchor_idx, attacker_port, prof)
            if move is not None:
                stats.observations_attributed += 1

            attacker_char_id = fields(rows[anchor_idx], attacker_block).get("char_id")
            defender_char_id = fields(rows[anchor_idx], defender_block).get("char_id")
            attacker_char = (
                prof.char_name(prof.canon_char_id(attacker_char_id))
                if attacker_char_id is not None else None
            )
            defender_char = (
                prof.char_name(prof.canon_char_id(defender_char_id))
                if defender_char_id is not None else None
            )

            observations.append(Observation(
                file=path.name, round_id=rid, frame=rows[anchor_idx].get("frame", anchor_idx),
                hits=hits, family=family, port=port,
                attacker_char=attacker_char, defender_char=defender_char,
                move=move, move_source=source,
                observed_advantage=observed_advantage,
                attacker_next_frame=attacker_next, defender_next_frame=defender_next,
                gap_px=gap_px,
            ))

    return observations, stats


def harvest_files(paths_and_profiles) -> tuple:
    """Harvest many files, each against its own profile (a caller-resolved
    `(path, GameProfile)` pair -- deliberately not a directory scanner: a
    recordings directory can mix ports (mk2's does), and picking the right
    profile per file is a `.meta.json`-reading job the caller (or
    `format_report`'s driver, see the bottom `main`) already has to do
    once. Returns `(all_observations, merged_stats)`."""
    all_obs: list = []
    stats = HarvestStats()
    for path, prof in paths_and_profiles:
        obs, s = harvest_file(path, prof)
        all_obs.extend(obs)
        stats.add(s)
    return all_obs, stats


def rank_candidates(observations: list, top_n: Optional[int] = None) -> list:
    """Candidate cells for the lab to measure next, ranked by how often real
    play produced a usable, attributed contact for that (family, port,
    attacker char, move) -- "what the next run should target," per the
    task. Unattributed observations still count toward `UNATTRIBUTED`
    cells (visible so a reader can see how much of real play the current
    attribution heuristic cannot yet name), but they are not useful lab
    targets by themselves."""
    counts: Counter = Counter()
    samples: dict = defaultdict(list)
    for o in observations:
        if o.observed_advantage is None or o.attacker_char is None:
            continue
        key = (o.family, o.port, o.attacker_char, o.move or "UNATTRIBUTED")
        counts[key] += 1
        if len(samples[key]) < 5:
            samples[key].append(o.observed_advantage)
    ranked = counts.most_common(top_n)
    return [
        CandidateCell(family=k[0], port=k[1], char=k[2], move=k[3],
                      count=n, sample_advantages=samples[k])
        for k, n in ranked
    ]


def load_measured_table(path) -> dict:
    """Read an exported `library/<family>/<port>.frames.json`
    (`framelab.export.export_frames`'s output) into
    `{(family, port, char, move): [row, ...]}`. Pure JSON read -- no
    `framelab.store` import, no SQLite, so this module has no path to the
    authoring database at all."""
    data = json.loads(Path(path).read_text())
    table: dict = defaultdict(list)
    for row in data.get("moves", []):
        key = (row["family"], row["port"], row["char"], row["move"])
        table[key].append(row)
    return table


def audit_against_table(observations: list, measured_table: dict,
                         tolerance: int = AUDIT_TOLERANCE_FRAMES) -> list:
    """§ "audit findings": an observation contradicts the table when its
    `observed_advantage` is more extreme (more negative -- "sooner," in the
    task's own phrasing) than the WORST of that cell's `on_hit`/`on_block`
    values, by more than `tolerance`. Comparing against the worst case
    (not just whichever of hit/block the observation "probably" was, which
    docs/frames.md §2 rule 6 says cannot be inferred from a health delta on
    this class of game) is the conservative choice: a finding here means
    the observation beat BOTH stored numbers, not just one interpretation
    of them."""
    findings = []
    for o in observations:
        if o.observed_advantage is None or o.move is None or o.attacker_char is None:
            continue
        key = (o.family, o.port, o.attacker_char, o.move)
        rows = measured_table.get(key)
        if not rows:
            continue
        bounds = [r["on_block"] for r in rows if r.get("on_block") is not None]
        bounds += [r["on_hit"] for r in rows if r.get("on_hit") is not None]
        if not bounds:
            continue
        worst = min(bounds)
        if o.observed_advantage < worst - tolerance:
            findings.append(AuditFinding(
                family=o.family, port=o.port, char=o.attacker_char, move=o.move,
                observed_advantage=o.observed_advantage, measured_bound=worst,
                tolerance=tolerance, file=o.file, round_id=o.round_id, frame=o.frame,
                detail=(
                    f"{o.attacker_char}/{o.move}: observed advantage "
                    f"{o.observed_advantage} is more negative than the measured "
                    f"table's worst case ({worst}) by more than {tolerance}f "
                    f"({o.file} round {o.round_id} @frame {o.frame}) -- "
                    "re-measurement candidate, not a verdict (see module docstring)"
                ),
            ))
    return findings


def format_report(observations: list, stats: HarvestStats,
                   candidates: list, findings: list,
                   arcade_x_caveat: bool = False) -> str:
    """Human-readable summary for the acceptance report -- counts first,
    then the ranked candidates, then audit findings, matching the task's
    honesty requirements (report what was skipped and why, never just the
    happy-path numbers)."""
    lines = []
    lines.append(f"files: {stats.files_total} total, "
                 f"{stats.files_skipped_unsupported_version} unsupported-version, "
                 f"{stats.files_skipped_no_health_field} no-health-field, "
                 f"{stats.files_skipped_error} error")
    lines.append(f"rounds usable: {stats.rounds_seen}")
    lines.append(f"contacts found: {stats.contacts_found}")
    lines.append(f"  skipped (pointer-unresolved x): {stats.contacts_skipped_pointer_unresolved_x}")
    lines.append(f"  skipped (attacker never observed acting again): "
                 f"{stats.contacts_skipped_no_action_attacker}")
    lines.append(f"  skipped (defender never observed acting again): "
                 f"{stats.contacts_skipped_no_action_defender}")
    lines.append(f"usable observations: {stats.observations_usable}")
    lines.append(f"  attributed: {stats.observations_attributed}")
    if stats.skipped_files:
        lines.append("skipped files:")
        for p, reason in stats.skipped_files:
            lines.append(f"  {p}: {reason}")
    if arcade_x_caveat:
        lines.append(
            "CAVEAT: mk2 arcade recordings on this machine source 'x' via the "
            "p1_x/p2_x globals, which docs/frames.md §4.2 rule 3 and "
            "library/mk2/mk2.md disprove as a position source (stale object-pool "
            "slot). gap_px values from mk2 arcade observations are reported but "
            "should not be trusted."
        )
    lines.append("")
    lines.append(f"top candidate cells (n={len(candidates)}):")
    for c in candidates[:20]:
        lines.append(
            f"  {c.family}/{c.port} {c.char} {c.move}: {c.count} contacts "
            f"(sample advantages: {c.sample_advantages})"
        )
    lines.append("")
    lines.append(f"audit findings (n={len(findings)}):")
    for f in findings[:50]:
        lines.append(f"  {f.detail}")
    return "\n".join(lines)


def main(argv=None) -> int:  # pragma: no cover - manual/report entry point
    """Minimal driver for running this module against a directory of
    recordings from the command line: `python -m shadow_train.framelab.harvest
    <dir> [<dir> ...] [--frames path/to/port.frames.json]`. Each `.jsonl`
    file's own `.meta.json` sidecar (v3) or filename-adjacent profile (v2)
    picks its profile -- this is a thin convenience wrapper, not a new CLI
    surface (that stays owned by `shadow_train.__main__`, out of this task's
    file scope)."""
    import sys

    from .. import profile as _profile

    argv = list(sys.argv[1:] if argv is None else argv)
    frames_path = None
    if "--frames" in argv:
        i = argv.index("--frames")
        frames_path = argv[i + 1]
        del argv[i:i + 2]
    dirs = [Path(a) for a in argv] or [Path("shadow/recordings")]

    files = []
    for d in dirs:
        files.extend(sorted(p for p in d.rglob("*.jsonl") if not p.name.endswith(".rounds.jsonl")))

    prof_cache: dict = {}

    def _prof_for(path: Path):
        meta_path = Path(str(path).removesuffix(".jsonl") + ".meta.json")
        family = port = None
        try:
            meta = json.loads(meta_path.read_text())
            family, port = meta.get("family"), meta.get("port")
        except (OSError, json.JSONDecodeError):
            pass
        # Best-effort family guess from the recordings/<family>/ layout when
        # no sidecar exists (older v2 asurabld files).
        if family is None:
            family = path.parent.name
        cache_key = (family, port)
        if cache_key not in prof_cache:
            candidates_dirs = [Path("library") / family / port] if port else []
            candidates_dirs.append(Path("library") / family)
            prof = None
            for d in candidates_dirs:
                try:
                    prof = _profile.load(d)
                    break
                except Exception:
                    continue
            prof_cache[cache_key] = prof
        return prof_cache[cache_key]

    pairs = []
    for f in files:
        prof = _prof_for(f)
        if prof is not None:
            pairs.append((f, prof))

    observations, stats = harvest_files(pairs)
    candidates = rank_candidates(observations)
    findings: list = []
    arcade_caveat = any(o.port == "arcade" and o.family == "mk2" for o in observations)
    if frames_path:
        table = load_measured_table(frames_path)
        findings = audit_against_table(observations, table)

    print(format_report(observations, stats, candidates, findings, arcade_caveat))
    return 0


if __name__ == "__main__":  # pragma: no cover
    import sys
    sys.exit(main())
