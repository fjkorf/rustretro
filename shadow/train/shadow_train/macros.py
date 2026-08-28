"""Train-side macro matcher (shadow/MACRO_ACTIONS.md §2/§4).

The train-side matcher is AUTHORITATIVE for decision labeling -- old
recordings made before the recorder's live matcher existed (§3) still label
correctly, because this module re-derives specials from the raw per-frame
`p1_input` mask stream rather than trusting any `p1_special` annotation
(annotations are for humans/coverage only, per §4).

This module is a frame-by-frame port of `src/macros.rs`'s `Matcher` -- same
state machine, same semantics, so Python and Rust produce byte-identical
completions off the same input stream. That parity is load-bearing: the
golden fixture (`shadow/train/tests/fixtures/matcher_golden.json`) pins one
truth both languages are tested against.

Vocabulary mirrors the contract exactly:

  - a MACRO is an ordered list of steps (`Macro`/`MacroStep`); a single-step
    macro is a chord (Reptile's arcade `slide` = back+LK+LP), multi-step is
    a motion (candidate `acid_spit` = F,F,HP).
  - `dirs` are SEMANTIC (back/forward/up/down), resolved against a per-frame
    facing sign `s` (>0 = facing right) the same way `dataset._move_class`
    resolves fwd/back -- both must agree so a matcher and dataset labeling
    call it the same move.
  - `press` names are attack-CLASS names (`attack_chords` keys). A step is
    SATISFIED at frame `i` when its `dirs` are held at `i` and every `press`
    class's full button chord is down AT FRAME `i` -- simultaneously, in
    that single frame. NO trailing "recently pressed" window: the game
    reads button state per frame, so simultaneity is the rule, not a
    lookback (a press-class onset that lands late still satisfies the chord
    the moment it overlaps the others still being held -- that overlap IS
    the simultaneity, not a tolerance grant).
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


_SEMANTIC_DIRS = ("back", "forward", "up", "down")


def _run_matcher(
    macros: list, masks: list, sides: list, attack_chords: dict, max_gap: int,
) -> list:
    """Frame-by-frame simulation of every macro's state machine across the
    whole stream at once -- a line-for-line port of `src/macros.rs`'s
    `Matcher::feed`, run in a loop instead of fed live, since Python gets
    the full `masks`/`sides` arrays up front. Returns `(frame, macro_name)`
    completions in the order they occur.

    `press` classes carry no memory (§2: satisfied iff the full chord is
    down THIS frame); `dirs` still track a per-frame onset so multi-step
    motions can require a FRESH tap on non-first steps (F,F needs a second
    forward TAP, not a continuous hold) -- that half of the contract is
    unchanged by the press-tolerance fix.
    """
    chord_mask_cache: dict = {}

    def chord_mask(cls: str) -> int:
        m = chord_mask_cache.get(cls)
        if m is None:
            m = _class_mask(cls, attack_chords)
            chord_mask_cache[cls] = m
        return m

    prev_dir = {d: False for d in _SEMANTIC_DIRS}
    dir_onset = {d: _NEVER for d in _SEMANTIC_DIRS}

    # Per-macro state: [next step index (== len(steps) means "cooldown:
    # waiting for the final step to release before re-arming"), activation
    # frame of the previous step / last reset].
    states = [[0, _NEVER] for _ in macros]
    events: list = []

    for i, (mask, s) in enumerate(zip(masks, sides)):
        for d in _SEMANTIC_DIRS:
            held = bool(mask & _dir_bit(d, s))
            if held and not prev_dir[d]:
                dir_onset[d] = i
            prev_dir[d] = held
        prev_mask = mask  # noqa: F841 (kept for parity/readability with Rust)

        def held_now(step: MacroStep) -> bool:
            if not all(prev_dir[d] for d in step.dirs):
                return False
            return all(mask & chord_mask(cls) == chord_mask(cls) for cls in step.press)

        def sat(step: MacroStep, activation: int, first: bool) -> bool:
            if not held_now(step):
                return False
            if not step.press or not first:
                onset_max = _NEVER
                for d in step.dirs:
                    o = dir_onset[d]
                    if o <= activation:
                        return False  # stale hold, not a fresh tap
                    onset_max = max(onset_max, o)
                return onset_max <= activation + max_gap
            return True

        for mi, macro in enumerate(macros):
            st = states[mi]
            n_steps = len(macro.steps)

            if st[0] == n_steps:
                # Cooldown: this macro just completed. Re-arm only once its
                # final step releases -- holding the chord is ONE input, so
                # it must not multiply-count completions (contract §2).
                if not held_now(macro.steps[-1]):
                    st[0], st[1] = 0, i
                continue

            if sat(macro.steps[st[0]], st[1], st[0] == 0):
                st[1] = i
                st[0] += 1
                if st[0] == n_steps:
                    events.append((i, macro.name))
                    # stays at step == n_steps (cooldown) -- see above.
            elif st[0] > 0:
                # A fresh step-0 satisfaction mid-macro restarts the window;
                # otherwise a blown gap resets to neutral.
                if sat(macro.steps[0], i - 1, True):
                    st[0], st[1] = 1, i
                elif i > st[1] + max_gap:
                    st[0], st[1] = 0, i

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
