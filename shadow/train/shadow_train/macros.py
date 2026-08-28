"""Train-side macro matcher (shadow/MACRO_ACTIONS.md §2/§4).

The train-side matcher is AUTHORITATIVE for decision labeling -- old
recordings made before the recorder's live matcher existed (§3) still label
correctly, because this module re-derives specials from the raw per-frame
`p1_input` mask stream rather than trusting any `p1_special` annotation
(annotations are for humans/coverage only, per §4).

Vocabulary mirrors the contract exactly:

  - a MACRO is an ordered list of steps (`Macro`/`MacroStep`); a single-step
    macro is a chord (Reptile's arcade `slide` = back+LK+LP), multi-step is
    a motion (candidate `acid_spit` = F,F,HP).
  - `dirs` are SEMANTIC (back/forward/up/down), resolved against a per-frame
    facing sign `s` (>0 = facing right) the same way `dataset._move_class`
    resolves fwd/back -- both must agree so a matcher and dataset labeling
    call it the same move.
  - `press` names are attack-CLASS names (`attack_chords` keys); a step's
    chord is "down" at frame `i` when, within the trailing `chord_tolerance`
    frames ending at `i` (inclusive), every press class's full button chord
    was held on at least one frame in that window -- this is what makes a
    2-frame-staggered LK-then-LP still read as one chord (§2's tolerance
    bullet) without demanding literal single-frame simultaneity.
  - between steps, at most `max_gap` frames may elapse from one step's
    completion frame to the next step's completion frame.

This module has no profile.py import (keeps it a pure function library,
unit-testable on synthetic mask streams with hand-built `attack_chords`
dicts) -- callers (dataset.py) pass the loaded profile's own `attack_chords`
and `special_inputs` data in.
"""

from __future__ import annotations

from dataclasses import dataclass

__all__ = [
    "CHORD_TOLERANCE", "MAX_GAP", "MacroStep", "Macro",
    "compile_macros", "find_macro_completions", "match_all",
]

CHORD_TOLERANCE = 3   # frames a step's press events may be staggered across
MAX_GAP = 12          # max frames between one step's completion and the next

# RETRO joypad button -> mask bit (same table as shadow_train.re.BUTTON_MASKS
# / src/lua_engine.rs's input.set -- duplicated here, not imported, so this
# module stays a standalone function library with no dependency on the MCP
# client re.py pulls in).
_BUTTON_MASKS = {
    "b": 0x1, "y": 0x2, "select": 0x4, "start": 0x8,
    "up": 0x10, "down": 0x20, "left": 0x40, "right": 0x80,
    "a": 0x100, "x": 0x200, "l": 0x400, "r": 0x800,
}


@dataclass(frozen=True)
class MacroStep:
    dirs: tuple  # semantic: subset of ("back", "forward", "up", "down")
    press: tuple  # attack-class names, all pressed together
    frames: int = 3  # execution hold length; see the module docstring for
                     # why the matcher does not additionally require this
                     # many CONSECUTIVE dir-held frames (see dataset.py's
                     # integration comment / the final report's contract-
                     # ambiguity note)


@dataclass(frozen=True)
class Macro:
    name: str
    steps: tuple  # tuple[MacroStep, ...]; len 1 = chord, len > 1 = motion


def compile_macros(special_inputs_for_char: dict) -> list:
    """`{move_name: [step_dict, ...]}` (a `GameProfile.special_inputs[char]`
    value, or the equivalent hand-built dict in a test) -> `[Macro, ...]`."""
    macros = []
    for name, steps in special_inputs_for_char.items():
        compiled = tuple(
            MacroStep(
                dirs=tuple(s.get("dirs", ())),
                press=tuple(s.get("press", ())),
                frames=int(s.get("frames", 3)),
            )
            for s in steps
        )
        macros.append(Macro(name=name, steps=compiled))
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


def _step_satisfied_at(
    step: MacroStep, i: int, masks: list, sides: list,
    attack_chords: dict, chord_tolerance: int,
) -> bool:
    """Is `step` complete AT frame `i` (§2 tolerance semantics)? `dirs` are
    checked at `i` itself (a held direction, current); `press` classes are
    each satisfied if their full chord was down on any frame within
    `[i - chord_tolerance, i]` (a "recently pressed" window, not necessarily
    the same single frame -- this is the staggered-press tolerance)."""
    if step.dirs:
        s = sides[i]
        m = masks[i]
        if not all(m & _dir_bit(d, s) for d in step.dirs):
            return False
    if step.press:
        window_lo = max(0, i - chord_tolerance)
        chord_masks = [_class_mask(cls, attack_chords) for cls in step.press]
        seen = [False] * len(step.press)
        for j in range(window_lo, i + 1):
            mj = masks[j]
            for k, bit in enumerate(chord_masks):
                if not seen[k] and (mj & bit) == bit:
                    seen[k] = True
        if not all(seen):
            return False
    return True


def _find_one_completion(
    macro: Macro, masks: list, sides: list, attack_chords: dict,
    chord_tolerance: int, max_gap: int, start: int,
):
    """First occurrence of `macro` completing at or after frame `start` ->
    its last step's completion frame, or None."""
    n = len(masks)
    step_completion = None
    for step_idx, step in enumerate(macro.steps):
        if step_idx == 0:
            lo, hi = start, n - 1
        else:
            lo, hi = step_completion, min(n - 1, step_completion + max_gap)
        found = None
        for i in range(lo, hi + 1):
            if _step_satisfied_at(step, i, masks, sides, attack_chords, chord_tolerance):
                found = i
                break
        if found is None:
            return None
        step_completion = found
    return step_completion


def find_macro_completions(
    macro: Macro, masks: list, sides: list, attack_chords: dict,
    chord_tolerance: int = CHORD_TOLERANCE, max_gap: int = MAX_GAP,
) -> list:
    """All of `macro`'s non-overlapping completion frames across the stream,
    in order. Resumes each search `chord_tolerance + 1` frames past the
    previous completion, not just +1 -- `_step_satisfied_at`'s trailing
    "recently pressed" window means the SAME physical button-down event can
    still satisfy the chord for up to `chord_tolerance` more frames after the
    earliest completion; skipping past that window is what keeps one press
    of a chord from being reported as several consecutive completions."""
    out = []
    start = 0
    n = len(masks)
    while start < n:
        c = _find_one_completion(macro, masks, sides, attack_chords, chord_tolerance, max_gap, start)
        if c is None:
            break
        out.append(c)
        start = c + chord_tolerance + 1
    return out


def match_all(
    macros: list, masks: list, sides: list, attack_chords: dict,
    chord_tolerance: int = CHORD_TOLERANCE, max_gap: int = MAX_GAP,
) -> list:
    """`(completion_frame, macro_name)` for every macro's every completion,
    frame-order sorted (ties broken by name for determinism)."""
    events = []
    for macro in macros:
        for f in find_macro_completions(macro, masks, sides, attack_chords, chord_tolerance, max_gap):
            events.append((f, macro.name))
    events.sort()
    return events
