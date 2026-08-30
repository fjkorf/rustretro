"""Train-side macro matcher (shadow/MACRO_ACTIONS.md §2/§4/§10).

The train-side matcher is AUTHORITATIVE for decision labeling -- old
recordings made before the recorder's live matcher existed (§3) still label
correctly, because this module re-derives specials from the raw per-frame
`p1_input` mask stream rather than trusting any `p1_special` annotation
(annotations are for humans/coverage only, per §4).

This module is a frame-by-frame port of `src/macros.rs`'s `Matcher` -- same
state machine, same semantics, so Python and Rust produce byte-identical
completions off the same input stream. That parity is load-bearing: the
golden fixtures (`shadow/train/tests/fixtures/matcher_golden.json` for §2,
`shadow/train/tests/fixtures/macro_ext_golden.json` for §10) pin one truth
both languages are tested against.

Vocabulary mirrors the contract exactly:

  - a MACRO is an ordered list of steps (`Macro`/`MacroStep`); a single-step
    macro is a chord (Reptile's arcade `slide` = back+LK+LP), multi-step is
    a motion (candidate `acid_spit` = F,F,HP).
  - `dirs` are SEMANTIC (back/forward/up/down), resolved against a per-frame
    facing sign `s` (>0 = facing right) the same way `dataset._move_class`
    resolves fwd/back -- both must agree so a matcher and dataset labeling
    call it the same move. §10.2: once a macro's first step satisfies, its
    facing is PINNED for the rest of that attempt -- a mid-macro side swap
    (Mileena's Teleport Kick) must not reinterpret "forward" partway through.
  - `press` names are attack-CLASS names (`attack_chords` keys). A step is
    SATISFIED at frame `i` when its `dirs` are held at `i` and every `press`
    class's full button chord is down AT FRAME `i` -- simultaneously, in
    that single frame. NO trailing "recently pressed" window: the game
    reads button state per frame, so simultaneity is the rule, not a
    lookback (a press-class onset that lands late still satisfies the chord
    the moment it overlaps the others still being held -- that overlap IS
    the simultaneity, not a tolerance grant).
  - §10.1 adds two more step KINDS (mutually exclusive with `press`, and
    with each other): `hold` (satisfied only once its chord has been down
    `min_frames` CONTINUOUS frames -- a release before that FAILS the whole
    macro) and `release` (satisfied on the FALLING edge of its chord).
    "Completion fires on the edge the FINAL step names" -- a macro ending in
    a `release` step completes on a release, not a press. `while_held` is an
    extra chord ANDed into any step's satisfaction regardless of kind -- the
    step-scoped stand-in for a hold spanning other steps (Reptile's
    Invisibility: Block held across `U U D`).
  - a macro COMPLETES on the rising edge of its final step's satisfaction
    (satisfied now, not satisfied last frame) -- one input is one move, so a
    chord held for 50 frames fires once, not once per frame. After firing,
    it re-arms only once the final step stops being satisfied (release), not
    after a fixed frame offset.
  - between steps of a multi-step motion, at most `max_gap` frames may
    elapse from one step's completion frame to the next step's completion
    frame (unchanged).

This module has no profile.py import (keeps it a pure function library,
unit-testable on synthetic mask streams with hand-built `attack_chords`
dicts) -- callers (dataset.py) pass the loaded profile's own `attack_chords`
and `special_inputs` data in.
"""

from __future__ import annotations

from dataclasses import dataclass

__all__ = [
    "MAX_GAP", "MacroStep", "Macro",
    "compile_macros", "find_macro_completions", "match_all",
]

MAX_GAP = 12          # max frames between one step's completion and the next
_NEVER = -1           # sentinel onset/activation value meaning "not yet"

# RETRO joypad button -> mask bit (same table as shadow_train.re.BUTTON_MASKS
# / src/lua_engine.rs's input.set -- duplicated here, not imported, so this
# module stays a standalone function library with no dependency on the MCP
# client re.py pulls in).
_BUTTON_MASKS = {
    "b": 0x1, "y": 0x2, "select": 0x4, "start": 0x8,
    "up": 0x10, "down": 0x20, "left": 0x40, "right": 0x80,
    "a": 0x100, "x": 0x200, "l": 0x400, "r": 0x800,
}

# The four physical direction bits a macro's `dirs` can ever resolve to --
# tracked in PHYSICAL space (§10.2), not semantic space: onset/held-ness of
# "the Right button" doesn't depend on facing, only WHICH semantic label
# ("forward" vs "back") a given facing maps it to at query time.
_PHYS_DIR_BITS = (_BUTTON_MASKS["up"], _BUTTON_MASKS["down"],
                   _BUTTON_MASKS["left"], _BUTTON_MASKS["right"])


@dataclass(frozen=True)
class MacroStep:
    dirs: tuple  # semantic: subset of ("back", "forward", "up", "down")
    press: tuple  # attack-class names, all pressed together (Normal kind)
    frames: int = 3  # execution hold length; see the module docstring for
                     # why the matcher does not additionally require this
                     # many CONSECUTIVE dir-held frames (see dataset.py's
                     # integration comment / the final report's contract-
                     # ambiguity note)
    hold: tuple = ()      # §10.1 Hold kind: chord classes, held continuously
    release: tuple = ()   # §10.1 Release kind: chord classes, falling edge
    while_held: tuple = ()  # extra chord ANDed in regardless of kind
    min_frames: int = 0   # Hold kind only: required continuous-hold frames
    kind: str = "normal"  # "normal" | "hold" | "release" -- derived, not
                          # meant to be passed by callers (compile_macros
                          # sets it from which of press/hold/release is
                          # non-empty)


@dataclass(frozen=True)
class Macro:
    name: str
    steps: tuple  # tuple[MacroStep, ...]; len 1 = chord, len > 1 = motion


def compile_macros(special_inputs_for_char: dict) -> list:
    """`{move_name: [step_dict, ...]}` (a `GameProfile.special_inputs[char]`
    value, or the equivalent hand-built dict in a test) -> `[Macro, ...]`.

    Mirrors `src/macros.rs::compile`'s shape checks (§10.1): `press`/`hold`/
    `release` are mutually exclusive per step (they name its KIND), a `hold`
    step needs a positive `min_frames`, and a step needs at least one of
    dirs/press/hold/release (bare `while_held` is not itself a step).
    """
    macros = []
    for name, steps in special_inputs_for_char.items():
        compiled = []
        for s in steps:
            dirs = tuple(s.get("dirs", ()))
            press = tuple(s.get("press", ()))
            hold = tuple(s.get("hold", ()))
            release = tuple(s.get("release", ()))
            while_held = tuple(s.get("while_held", ()))
            min_frames = s.get("min_frames")
            if not (dirs or press or hold or release):
                raise ValueError(f"macro {name!r}: empty step")
            kinds_present = sum(1 for k in (press, hold, release) if k)
            if kinds_present > 1:
                raise ValueError(f"macro {name!r}: step mixes press/hold/release -- pick one")
            if hold:
                kind = "hold"
                if not min_frames or min_frames <= 0:
                    raise ValueError(f"macro {name!r}: hold step needs a positive min_frames")
            elif release:
                kind = "release"
                min_frames = 0
            else:
                kind = "normal"
                min_frames = 0
            compiled.append(MacroStep(
                dirs=dirs, press=press, frames=int(s.get("frames", 3)),
                hold=hold, release=release, while_held=while_held,
                min_frames=int(min_frames or 0), kind=kind,
            ))
        macros.append(Macro(name=name, steps=tuple(compiled)))
    return macros


def _dir_bit(direction: str, s: int) -> int:
    """Semantic direction -> mask bit, side-resolved by facing sign `s` (>0
    = facing right) -- same resolution `dataset._move_class` uses for fwd/
    back, so both readers call a held direction the same thing."""
    if direction == "forward":
        return _BUTTON_MASKS["right"] if s > 0 else _BUTTON_MASKS["left"]
    if direction == "back":
        return _BUTTON_MASKS["left"] if s > 0 else _BUTTON_MASKS["right"]
    if direction == "up":
        return _BUTTON_MASKS["up"]
    if direction == "down":
        return _BUTTON_MASKS["down"]
    raise ValueError(f"unknown macro direction {direction!r} (valid: back/forward/up/down)")


def _class_mask(cls: str, attack_chords: dict) -> int:
    mask = 0
    for name in attack_chords[cls]:
        mask |= _BUTTON_MASKS[name]
    return mask


def _run_matcher(
    macros: list, masks: list, sides: list, attack_chords: dict, max_gap: int,
) -> list:
    """Frame-by-frame simulation of every macro's state machine across the
    whole stream at once -- a line-for-line port of `src/macros.rs`'s
    `Matcher::feed`, run in a loop instead of fed live, since Python gets
    the full `masks`/`sides` arrays up front. Returns `(frame, macro_name)`
    completions in the order they occur.

    `press`/`hold`/`release` classes carry no memory of their own (§2:
    satisfied iff the full chord reads the required way THIS frame -- down
    for press/hold, a down-then-up transition for release); `dirs` still
    track a per-frame onset, in PHYSICAL space (§10.2), so multi-step
    motions can require a FRESH tap on non-first steps (F,F needs a second
    forward TAP, not a continuous hold) and so a macro's facing can be
    PINNED at its first step's satisfaction frame and reused (via the same
    physical onset table) for the rest of the attempt.
    """
    chord_mask_cache: dict = {}

    def chord_mask(cls: str) -> int:
        m = chord_mask_cache.get(cls)
        if m is None:
            m = _class_mask(cls, attack_chords)
            chord_mask_cache[cls] = m
        return m

    def chord_down(classes: tuple, m: int) -> bool:
        return all(m & chord_mask(c) == chord_mask(c) for c in classes)

    # Physical (facing-independent) rising-edge onset per direction bit --
    # replaces a semantic-space table so a macro's PIN and the live table
    # can share the same onset data (§10.2).
    bit_onset = {b: _NEVER for b in _PHYS_DIR_BITS}
    prev_bits = {b: False for b in _PHYS_DIR_BITS}

    # Per-macro state: [next step index (== len(steps) means "cooldown:
    # waiting for the final step to release before re-arming"), activation
    # frame of the previous step / last reset, hold_onset (§10.1, _NEVER
    # when not currently accumulating a Hold step's continuous hold),
    # pinned_facing (§10.2, None until the attempt's first step satisfies)].
    states = [[0, _NEVER, _NEVER, None] for _ in macros]
    events: list = []

    prev_mask = 0
    for i, (mask, s) in enumerate(zip(masks, sides)):
        old_mask = prev_mask  # this frame's "last frame" for release edges
        for b in _PHYS_DIR_BITS:
            held = bool(mask & b)
            if held and not prev_bits[b]:
                bit_onset[b] = i
            prev_bits[b] = held
        prev_mask = mask

        def dirs_held(dirs: tuple, facing: int) -> bool:
            return all(mask & _dir_bit(d, facing) for d in dirs)

        def held_now(step: MacroStep, facing: int) -> bool:
            if not dirs_held(step.dirs, facing):
                return False
            if not chord_down(step.while_held, mask):
                return False
            if step.kind == "hold":
                return chord_down(step.hold, mask)
            if step.kind == "release":
                return chord_down(step.release, old_mask) and not chord_down(step.release, mask)
            return chord_down(step.press, mask)

        def sat(step: MacroStep, activation: int, first: bool, facing: int) -> bool:
            if not held_now(step, facing):
                return False
            if not step.press or not first:
                onset_max = _NEVER
                for d in step.dirs:
                    o = bit_onset[_dir_bit(d, facing)]
                    if o <= activation:
                        return False  # stale hold, not a fresh tap
                    onset_max = max(onset_max, o)
                return onset_max <= activation + max_gap
            return True

        for mi, macro in enumerate(macros):
            st = states[mi]
            n_steps = len(macro.steps)
            facing_for = lambda pin: pin if pin is not None else s  # noqa: E731

            if st[0] == n_steps:
                # Cooldown: this macro just completed. Re-arm only once its
                # final step releases -- holding the chord is ONE input, so
                # it must not multiply-count completions (contract §2).
                facing = facing_for(st[3])
                if not held_now(macro.steps[-1], facing):
                    states[mi] = [0, i, _NEVER, None]
                continue

            cur = macro.steps[st[0]]

            if cur.kind == "hold":
                # §10.1: satisfied only once held `min_frames` CONTINUOUS
                # frames; a release before that FAILS the whole macro (not
                # a soft reset-and-maybe-restart -- parking short of the
                # threshold is not progress).
                facing = facing_for(st[3])
                if held_now(cur, facing):
                    if st[2] == _NEVER:
                        st[2] = i
                    if i - st[2] + 1 >= cur.min_frames:
                        if st[0] == 0:
                            st[3] = facing
                        st[1] = i
                        st[0] += 1
                        st[2] = _NEVER
                        if st[0] == n_steps:
                            events.append((i, macro.name))
                elif st[2] != _NEVER:
                    states[mi] = [0, i, _NEVER, None]
                continue

            first = st[0] == 0
            facing = facing_for(st[3])
            if sat(cur, st[1], first, facing):
                if first:
                    st[3] = facing
                st[1] = i
                st[0] += 1
                if st[0] == n_steps:
                    events.append((i, macro.name))
                    # stays at step == n_steps (cooldown) -- see above.
            elif st[0] > 0:
                # A fresh step-0 satisfaction mid-macro restarts the window;
                # re-pinning facing to THIS restart's live side (§10.2 -- a
                # restart is a new attempt). Skipped when step 0 is Hold
                # kind: its `held_now` is a continuous "still down"
                # condition, not a discrete tap, so re-checking it here
                # would spuriously "restart" every frame the chord stays
                # down. Otherwise a blown gap resets to neutral.
                if macro.steps[0].kind != "hold" and sat(macro.steps[0], i - 1, True, s):
                    states[mi] = [1, i, _NEVER, s]
                elif i > st[1] + max_gap:
                    states[mi] = [0, i, _NEVER, None]

    return events


def find_macro_completions(
    macro: Macro, masks: list, sides: list, attack_chords: dict,
    max_gap: int = MAX_GAP,
) -> list:
    """All of `macro`'s completion frames across the stream, in order."""
    return [f for f, _ in _run_matcher([macro], masks, sides, attack_chords, max_gap)]


def match_all(
    macros: list, masks: list, sides: list, attack_chords: dict,
    max_gap: int = MAX_GAP,
) -> list:
    """`(completion_frame, macro_name)` for every macro's every completion,
    frame-order sorted (ties broken by name for determinism)."""
    events = _run_matcher(macros, masks, sides, attack_chords, max_gap)
    events.sort()
    return events
