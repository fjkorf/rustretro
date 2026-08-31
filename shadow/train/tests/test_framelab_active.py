"""Unit tests for `framelab.active` — the teleport-probed ACTIVE window.

Everything runs against `HitboxToy`, a 1-D fighter whose hitbox is live over a
declared frame range and out to a declared reach. The toy exists to reproduce,
without an emulator, the four things the live spike had to rule out before the
window could be believed:

  1. the write STICKS (the toy's x is authoritative, and a sub-floor write is
     pushed back apart — so the floor refusal has something to refuse);
  2. the teleported body is a valid target (the toy checks distance, not
     provenance);
  3. the window must not be the defender's animation phase — the toy carries a
     free-running idle counter that the sweep must never end up measuring, and
     `fringe_from` makes the outermost pixels arrive late exactly as MK2's
     extending limb does;
  4. the teleport must not alter the interaction — a null write is inert.

The refusal rules live in `window_from_trials`, which is pure, so they are
tested directly on synthesized predicates as well.
"""

from __future__ import annotations

import unittest

from shadow_train.framelab.active import (
    CAP_MARGIN,
    MIN_TELEPORT_FRAME,
    ActiveError,
    ActiveWindow,
    ClippedWindowError,
    CollisionFloorError,
    GapDisagreementError,
    NonMonotoneWindowError,
    PositionIO,
    TeleportTrial,
    VariantDriftError,
    derive_recovery,
    measure_active,
    sweep_active_window,
    teleport_trial,
    window_from_trials,
)
from shadow_train.framelab.probe import MoveScript, Rig, ScriptStep


# ── the toy ───────────────────────────────────────────────────────────────


class HitboxToy:
    """Attacker at x=0, defender at x=`gap`. Pressing ATTACK at frame `f`
    starts a move whose hitbox is live on animation frames
    `[active_from, active_to]` and reaches `reach` px (or only `fringe_reach`
    px until animation frame `fringe_from`). Contact costs the defender
    `damage`, or `close_damage` when the move was STARTED inside `close_px` —
    the proximity variant, which is what makes an early teleport dangerous.
    """

    ATTACK = "atk"
    FLOOR = 62

    def __init__(self, *, gap=192, active_from=11, active_to=16, reach=84,
                 fringe_reach=None, fringe_from=None, damage=11,
                 close_damage=24, close_px=64, resolve_at=2):
        self.cfg = dict(
            active_from=active_from, active_to=active_to, reach=reach,
            fringe_reach=fringe_reach, fringe_from=fringe_from,
            damage=damage, close_damage=close_damage, close_px=close_px,
            resolve_at=resolve_at,
        )
        self.start_gap = gap
        self.writes_enabled = False
        self.calls = []
        self._reset()

    def _reset(self):
        self.x = {0: 0, 1: self.start_gap}
        self.health = {0: 100, 1: 100}
        self.held = {0: (), 1: ()}
        self.frame = 0
        self.attack_at = None
        self.variant = None      # locked at `resolve_at` frames into the move
        self.spent = False
        self.idle = 7            # free-running, never the answer

    # ── mechanics ────────────────────────────────────────────────────────
    def _step(self):
        self.frame += 1
        self.idle += 1
        if self.ATTACK in self.held[0] and self.attack_at is None:
            self.attack_at = self.frame - 1
        # anti-overlap: the bodies separate at 6 px/frame below the floor
        if self.x[1] - self.x[0] < self.FLOOR:
            self.x[1] = min(self.x[0] + self.FLOOR, self.x[1] + 6)
        if self.attack_at is None:
            return
        a = self.frame - self.attack_at
        c = self.cfg
        if a == c["resolve_at"]:
            self.variant = "close" if self.x[1] - self.x[0] <= c["close_px"] else "far"
        if self.spent or not (c["active_from"] <= a <= c["active_to"]):
            return
        reach = c["reach"]
        if c["fringe_reach"] is not None and a < c["fringe_from"]:
            reach = c["fringe_reach"]
        if self.x[1] - self.x[0] <= reach:
            self.health[1] -= (
                c["close_damage"] if self.variant == "close" else c["damage"]
            )
            self.spent = True

    # ── the client contract `LabSession` uses ────────────────────────────
    def call(self, tool, **kw):
        self.calls.append((tool, kw))
        if tool == "enable_writes":
            self.writes_enabled = True
            return {"ok": True, "writes_enabled": True}
        if tool == "load_state":
            self._reset()
            return {"ok": True, "loaded": True}
        if tool == "write_memory":
            if not self.writes_enabled:
                return {"ok": False, "error": "writes not enabled"}
            self.x[kw["addr"]] = kw["value"]
            return {"ok": True, "wrote": True}
        if tool == "read_memory":
            return {"ok": True, "value": self.x[kw["addr"]]}
        raise AssertionError(f"unexpected tool {tool!r}")


class ToySession:
    """The `LabSession` surface `active.teleport_trial` actually touches."""

    def __init__(self, game: HitboxToy):
        self.game = game
        self.loads = 0

    def load_state(self, arena):
        self.game.call("load_state", path=arena)
        self.loads += 1

    def run_frames(self, count, holds=None):
        for port, buttons in (holds or {}).items():
            self.game.held[port] = tuple(buttons)
        for _ in range(count):
            self.game._step()

    def release_all_ports(self):
        self.game.held = {0: (), 1: ()}


def toy_rig():
    return Rig(arena="toy.state", attacker_port=0, defender_port=1,
               guard_buttons=("grd",))


def toy_script(lead=0):
    return MoveScript(
        name="HP",
        steps=(ScriptStep(frames=2, buttons=(HitboxToy.ATTACK,)),),
        lead_in=(ScriptStep(frames=lead, buttons=("down",)),) if lead else (),
    )


def toy_positions(game, *, side="defender", floor=HitboxToy.FLOOR):
    """`PositionIO` over the toy — the same shape `from_fighters` builds, with
    the two x accessors injected so no emulator or profile is involved."""
    game.writes_enabled = True

    def read_gap(_s):
        return game.x[1] - game.x[0]

    def write_gap(_s, gap):
        if side == "defender":
            game.call("write_memory", addr=1, value=game.x[0] + gap)
        else:
            game.call("write_memory", addr=0, value=game.x[1] - gap)

    return PositionIO(read_gap=read_gap, write_gap=write_gap,
                      collision_floor_px=floor, side=side)


def contact_read(_s, game=None):
    return game.health[1]


def run_sweep(game, **kw):
    sess = ToySession(game)
    return sweep_active_window(
        sess, rig=toy_rig(), script=kw.pop("script", toy_script()),
        positions=kw.pop("positions", toy_positions(game)),
        contact_read=lambda s: game.health[1], **kw,
    )


# ── the window itself ─────────────────────────────────────────────────────


class TestSweep(unittest.TestCase):
    def test_window_matches_the_toy_and_reproduces_the_live_shape(self):
        """[11,16] at a deep gap — the live far-HP result, and the predicate is
        one contiguous run of TRUEs ending at N = last_active - 1."""
        game = HitboxToy(active_from=11, active_to=16)
        w = run_sweep(game, target_gap=76, max_search=30)
        self.assertEqual((w.first_active_frame, w.last_active_frame), (11, 16))
        self.assertEqual(w.active, 6)
        self.assertEqual(w.damage, 11)
        self.assertEqual(w.predicate, "T" * 14 + "F" * 15)
        self.assertEqual(w.trials[-1].teleport_at, 30)

    def test_contact_frame_is_max_of_first_active_and_teleport_plus_one(self):
        game = HitboxToy(active_from=11, active_to=16)
        w = run_sweep(game, target_gap=76, max_search=30)
        by_n = {t.teleport_at: t.contact_frame for t in w.trials}
        self.assertEqual(by_n[5], 11)     # hitbox waits for the body
        self.assertEqual(by_n[13], 14)    # body waits for the hitbox
        self.assertEqual(by_n[15], 16)
        self.assertIsNone(by_n[16])

    def test_window_is_a_property_of_the_attack_not_the_idle_phase(self):
        """docs/frames.md kill criterion 3: delaying the attack by k idle
        frames must move the window by exactly k and never resize it."""
        for k in range(0, 6):
            game = HitboxToy(active_from=11, active_to=16)
            w = run_sweep(game, target_gap=76, max_search=36, extra_lead=k)
            self.assertEqual(
                (w.first_active_frame - k, w.last_active_frame - k), (11, 16),
                f"window moved by something other than k={k}",
            )

    def test_input_relative_subtracts_the_stance_lead_in(self):
        game = HitboxToy(active_from=20, active_to=24)
        w = run_sweep(game, target_gap=76, max_search=40, script=toy_script(lead=6))
        self.assertEqual((w.first_active_frame, w.last_active_frame), (26, 30))
        self.assertEqual(w.input_relative, (20, 24))

    def test_attacker_side_teleport_gives_the_same_window(self):
        """The independent write target: a different body, the opposite sign."""
        deep = run_sweep(HitboxToy(), target_gap=76, max_search=30)
        g2 = HitboxToy()
        att = run_sweep(g2, target_gap=76, max_search=30,
                        positions=toy_positions(g2, side="attacker"))
        self.assertEqual(
            (deep.first_active_frame, deep.last_active_frame),
            (att.first_active_frame, att.last_active_frame),
        )
        self.assertEqual(att.side, "attacker")

    def test_a_null_write_leaves_a_whiff_a_whiff(self):
        """Kill criterion 4: the teleport must not be the thing that connects."""
        game = HitboxToy(gap=192)
        t = teleport_trial(
            ToySession(game), rig=toy_rig(), script=toy_script(),
            positions=toy_positions(game), contact_read=lambda s: game.health[1],
            teleport_at=8, target_gap=192, frames=40,
        )
        self.assertIsNone(t.contact_frame)
        self.assertFalse(t.connected)


# ── the refusals ──────────────────────────────────────────────────────────


class TestRefusals(unittest.TestCase):
    def test_sub_floor_target_is_refused_before_any_frame_runs(self):
        game = HitboxToy()
        with self.assertRaises(CollisionFloorError):
            run_sweep(game, target_gap=40, max_search=30)
        self.assertEqual(ToySession(game).loads, 0)

    def test_teleport_before_the_variant_lock_is_refused(self):
        game = HitboxToy()
        with self.assertRaises(ClippedWindowError):
            teleport_trial(
                ToySession(game), rig=toy_rig(), script=toy_script(),
                positions=toy_positions(game),
                contact_read=lambda s: game.health[1],
                teleport_at=MIN_TELEPORT_FRAME - 1, target_gap=76, frames=40,
            )

    def test_close_variant_sweep_is_refused_as_variant_drift(self):
        """A sweep whose early trials land inside the proximity bucket deals a
        different damage — which is a DIFFERENT MOVE, not a wider window."""
        game = HitboxToy(close_px=80, resolve_at=4)
        with self.assertRaises(VariantDriftError):
            run_sweep(game, target_gap=76, max_search=30)

    def test_non_monotone_predicate_is_refused(self):
        trials = [
            TeleportTrial(teleport_at=n, target_gap=76,
                          contact_frame=(12 if n in (2, 3, 7) else None),
                          damage=11 if n in (2, 3, 7) else None, frames=40)
            for n in range(2, 12)
        ]
        with self.assertRaises(NonMonotoneWindowError):
            window_from_trials(trials, move="HP", max_search=30)

    def test_window_clipped_at_the_low_end_is_refused(self):
        """Every trial connecting one frame after its own teleport means the
        hitbox was already out before we were allowed to look."""
        trials = [
            TeleportTrial(teleport_at=n, target_gap=76, contact_frame=n + 1,
                          damage=11, frames=40)
            for n in range(MIN_TELEPORT_FRAME, 12)
        ]
        with self.assertRaises(ClippedWindowError):
            window_from_trials(trials, move="HP", max_search=30)

    def test_window_capped_at_the_high_end_is_refused(self):
        trials = [
            TeleportTrial(teleport_at=n, target_gap=76, contact_frame=max(11, n + 1),
                          damage=11, frames=60)
            for n in range(MIN_TELEPORT_FRAME, 31)
        ]
        with self.assertRaises(ActiveError) as cm:
            window_from_trials(trials, move="HP", max_search=30,
                               cap_margin=CAP_MARGIN)
        self.assertIn("max_search", str(cm.exception))

    def test_no_contact_at_any_n_is_a_whiff_not_a_zero_window(self):
        trials = [
            TeleportTrial(teleport_at=n, target_gap=200, contact_frame=None,
                          damage=None, frames=40)
            for n in range(2, 20)
        ]
        with self.assertRaises(ActiveError) as cm:
            window_from_trials(trials, move="HP", max_search=30)
        self.assertIn("WHIFF", str(cm.exception))

    def test_a_move_that_connects_without_the_teleport_is_refused(self):
        trials = [
            TeleportTrial(teleport_at=n, target_gap=76, contact_frame=8, damage=11,
                          frames=40)
            for n in range(MIN_TELEPORT_FRAME, 12)
        ]
        with self.assertRaises(ActiveError) as cm:
            window_from_trials(trials, move="HP", max_search=30)
        self.assertIn("does not isolate", str(cm.exception))


# ── the fringe, and the defence against it ────────────────────────────────


class TestGapAgreement(unittest.TestCase):
    def test_fringe_gap_reports_a_late_start_and_is_caught_by_agreement(self):
        """The live failure: at the outermost pixels the limb is still
        extending, so the window LOOKS shorter and later. One gap cannot tell
        that from a real window; two can."""
        def game():
            return HitboxToy(active_from=11, active_to=16, reach=87,
                             fringe_reach=84, fringe_from=15)

        deep = run_sweep(game(), target_gap=76, max_search=30)
        fringe = run_sweep(game(), target_gap=86, max_search=30)
        self.assertEqual((deep.first_active_frame, deep.last_active_frame), (11, 16))
        self.assertEqual((fringe.first_active_frame, fringe.last_active_frame),
                         (15, 16))

        g = game()
        with self.assertRaises(GapDisagreementError):
            measure_active(
                ToySession(g), rig=toy_rig(), script=toy_script(),
                positions=toy_positions(g), contact_read=lambda s: g.health[1],
                target_gaps=(76, 86), max_search=30,
            )

    def test_agreeing_gaps_return_the_inner_window(self):
        g = HitboxToy(active_from=11, active_to=16, reach=84)
        w = measure_active(
            ToySession(g), rig=toy_rig(), script=toy_script(),
            positions=toy_positions(g), contact_read=lambda s: g.health[1],
            target_gaps=(66, 76, 83), max_search=30,
        )
        self.assertEqual((w.first_active_frame, w.last_active_frame), (11, 16))
        self.assertEqual(w.target_gap, 66)

    def test_one_gap_is_refused_when_agreement_is_required(self):
        g = HitboxToy()
        with self.assertRaises(ValueError):
            measure_active(
                ToySession(g), rig=toy_rig(), script=toy_script(),
                positions=toy_positions(g), contact_read=lambda s: g.health[1],
                target_gaps=(76,), max_search=30,
            )


# ── recovery / total ──────────────────────────────────────────────────────


class TestDerive(unittest.TestCase):
    def _win(self, first=11, last=16, lead=0):
        return ActiveWindow(move="HP", target_gap=76, first_active_frame=first,
                            last_active_frame=last, predicate="T", damage=11,
                            attack_input_frame=lead)

    def test_total_is_first_plus_active_plus_recovery(self):
        got = derive_recovery(self._win(), whiff_manifest_frame=21)
        self.assertEqual(got, {"first_active_frame": 11, "active": 6,
                               "last_active_frame": 16, "recovery": 5,
                               "total": 21})
        self.assertEqual(got["total"], got["last_active_frame"] + got["recovery"])
        self.assertEqual(
            got["total"],
            got["first_active_frame"] + got["active"] - 1 + got["recovery"],
        )

    def test_lead_in_is_subtracted_before_the_arithmetic(self):
        got = derive_recovery(self._win(first=20, last=24, lead=6),
                              whiff_manifest_frame=50)
        self.assertEqual(got["first_active_frame"], 14)
        self.assertEqual(got["last_active_frame"], 18)
        self.assertEqual(got["recovery"], 32)

    def test_total_before_the_last_active_frame_is_refused(self):
        with self.assertRaises(ActiveError):
            derive_recovery(self._win(), whiff_manifest_frame=15)


if __name__ == "__main__":  # pragma: no cover
    unittest.main()
