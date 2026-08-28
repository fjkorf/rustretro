"""shadow/MACRO_ACTIONS.md §2: the train-side macro matcher, unit-tested
against synthetic per-frame `p1_input` mask streams (pure functions, no
profile/recording I/O -- see shadow_train/macros.py's docstring).

Button bit reference (RETRO joypad order, matches shadow_train.re.BUTTON_
MASKS and dataset.py's BIT_* constants): b=0x1, y=0x2, select=0x4, start=0x8,
up=0x10, down=0x20, left=0x40, right=0x80, a=0x100, x=0x200, l=0x400, r=0x800.
"""

from __future__ import annotations

import unittest

from shadow_train.macros import Macro, MacroStep, compile_macros, find_macro_completions, match_all

LEFT, RIGHT = 0x40, 0x80
A, B, X, Y = 0x100, 0x1, 0x200, 0x2

# MK2 arcade's real attack_chords shape (library/mk2/mk2.profile.json):
# LP=b, LK=a, HP=y, HK=x. Genesis remaps the kick pair: LK=r, HK=l
# (library/mk2/genesis.profile.json).
ARCADE_CHORDS = {"LP": ["b"], "LK": ["a"], "HP": ["y"], "HK": ["x"]}
GENESIS_CHORDS = {"LP": ["b"], "LK": ["r"], "HP": ["y"], "HK": ["l"]}

# The contract's reference data (§2): Reptile's slide is a single-step chord,
# port-divergent encoding.
ARCADE_SLIDE = {"reptile": {"slide": [{"dirs": ["back"], "press": ["LK", "LP"], "frames": 4}]}}
GENESIS_SLIDE = {"reptile": {"slide": [{"dirs": ["back"], "press": ["LK", "HK"], "frames": 4}]}}


class ArcadeSlideStaggerTest(unittest.TestCase):
    """"the arcade slide (back+LK+LP with a staggered press) labels 'slide'"
    -- the game reads button STATE per frame (§2), so a chord completes the
    instant every class is simultaneously down, however far apart their
    onsets landed, as long as the first one is still HELD when the second
    arrives (no trailing "recently pressed" window; overlap is what counts).
    """

    def test_two_frame_stagger_still_completes_the_chord(self):
        macros = compile_macros(ARCADE_SLIDE["reptile"])
        n = 20
        masks = [LEFT] * n  # back held throughout
        # LK ("a") pressed frames 10-13 (held); LP ("b") pressed frames
        # 12-13 -- a 2-frame stagger between the two presses starting, but
        # LK is still held when LP arrives, so they overlap at frame 12.
        for i in range(10, 14):
            masks[i] |= A
        for i in (12, 13):
            masks[i] |= B
        sides = [1] * n  # facing right -> back = LEFT

        completions = find_macro_completions(macros[0], masks, sides, ARCADE_CHORDS)
        self.assertEqual(completions, [12])  # first frame both presses overlap

        events = match_all(macros, masks, sides, ARCADE_CHORDS)
        self.assertEqual(events, [(12, "slide")])

    def test_presses_that_never_overlap_do_not_complete(self):
        macros = compile_macros(ARCADE_SLIDE["reptile"])
        n = 20
        masks = [LEFT] * n
        masks[0] |= A
        masks[10] |= B  # A already released by the time B arrives -- never
                        # simultaneously down, so never a chord.
        sides = [1] * n
        self.assertEqual(find_macro_completions(macros[0], masks, sides, ARCADE_CHORDS), [])


class BareAttackDoesNotMatchTest(unittest.TestCase):
    """"a bare LP still labels LP" -- the matcher-level half: a lone LP tap
    (no back hold, no LK) must never register as the slide macro."""

    def test_lone_lp_never_completes_slide(self):
        macros = compile_macros(ARCADE_SLIDE["reptile"])
        n = 20
        masks = [0] * n
        masks[5] |= B  # bare LP, no back, no LK
        sides = [1] * n
        self.assertEqual(match_all(macros, masks, sides, ARCADE_CHORDS), [])

    def test_back_held_without_either_press_does_not_complete(self):
        macros = compile_macros(ARCADE_SLIDE["reptile"])
        n = 10
        masks = [LEFT] * n
        sides = [1] * n
        self.assertEqual(match_all(macros, masks, sides, ARCADE_CHORDS), [])


class FacingFlipTest(unittest.TestCase):
    """"facing-flipped back resolves correctly" -- `back` is semantic; when
    facing left (s<0), "back" means holding RIGHT, not LEFT."""

    def test_facing_left_resolves_back_to_right(self):
        macros = compile_macros(ARCADE_SLIDE["reptile"])
        n = 20
        masks = [RIGHT] * n  # holding RIGHT is "back" while facing left
        masks[8] |= A
        masks[9] |= A  # LK held through frame 9 so it overlaps LP
        masks[9] |= B
        sides = [-1] * n  # facing left
        self.assertEqual(match_all(macros, masks, sides, ARCADE_CHORDS), [(9, "slide")])

    def test_facing_left_holding_the_wrong_bit_does_not_complete(self):
        # Holding LEFT while facing left is FORWARD, not back -- must not match.
        macros = compile_macros(ARCADE_SLIDE["reptile"])
        n = 20
        masks = [LEFT] * n
        masks[8] |= A
        masks[9] |= B
        sides = [-1] * n
        self.assertEqual(match_all(macros, masks, sides, ARCADE_CHORDS), [])


class GenesisSlideEncodingTest(unittest.TestCase):
    """"genesis slide encoding labels on a genesis-shaped fixture" -- same
    move name, port-divergent buttons (LK+HK, not LK+LP), port-divergent
    button->name mapping (LK=r, HK=l instead of arcade's LK=a, LP=b)."""

    def test_genesis_button_pair_completes_the_genesis_macro(self):
        macros = compile_macros(GENESIS_SLIDE["reptile"])
        n = 20
        R, L = 0x800, 0x400  # r, l bits
        masks = [LEFT] * n
        masks[3] |= R  # LK on genesis
        masks[4] |= R  # LK held through frame 4 so it overlaps HK
        masks[4] |= L  # HK on genesis
        sides = [1] * n
        self.assertEqual(
            match_all(macros, masks, sides, GENESIS_CHORDS), [(4, "slide")]
        )

    def test_arcade_chord_shape_does_not_complete_on_genesis_encoding(self):
        # The arcade slide's LK+LP (a+b) pressed on a genesis-mapped macro
        # (which wants LK+HK = r+l) must not accidentally match.
        macros = compile_macros(GENESIS_SLIDE["reptile"])
        n = 20
        masks = [LEFT] * n
        masks[3] |= A
        masks[4] |= B
        sides = [1] * n
        self.assertEqual(match_all(macros, masks, sides, GENESIS_CHORDS), [])


class MultiStepMotionTest(unittest.TestCase):
    """Sanity check on the max_gap plumbing for multi-step macros (the
    contract's acid_spit candidate: forward, forward, HP) -- not one of the
    four required cases, but the matcher claims to support motions, not just
    chords, so it needs at least one exercise of the multi-step path."""

    def test_two_step_motion_within_max_gap_completes(self):
        macro = Macro(name="acid_spit", steps=(
            MacroStep(dirs=("forward",), press=(), frames=3),
            MacroStep(dirs=("forward",), press=("HP",), frames=3),
        ))
        n = 20
        masks = [0] * n
        masks[2] = RIGHT               # step 1: forward held
        masks[10] = RIGHT | Y          # step 2: forward + HP, well within max_gap=12
        sides = [1] * n
        self.assertEqual(
            find_macro_completions(macro, masks, sides, ARCADE_CHORDS), [10]
        )

    def test_two_step_motion_beyond_max_gap_does_not_complete(self):
        macro = Macro(name="acid_spit", steps=(
            MacroStep(dirs=("forward",), press=(), frames=3),
            MacroStep(dirs=("forward",), press=("HP",), frames=3),
        ))
        n = 30
        masks = [0] * n
        masks[0] = RIGHT
        masks[20] = RIGHT | Y  # 20 frames later -- past max_gap=12
        sides = [1] * n
        self.assertEqual(
            find_macro_completions(macro, masks, sides, ARCADE_CHORDS), []
        )


if __name__ == "__main__":
    unittest.main()
