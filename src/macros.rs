//! Macro actions (shadow/MACRO_ACTIONS.md): named specials as profile data,
//! plus the ONE matcher/executor pair every consumer shares.
//!
//! Design law: moves are not inputs. A move name ("slide") is family-level
//! vocabulary; its encoding is port-level `special_inputs` data. This module
//! holds no game knowledge — it compiles whatever steps the loaded profile
//! declares: semantic directions (`back`/`forward`/`up`/`down`, resolved
//! against live facing at match/execute time) and press CLASSES (compiled
//! through the port's `attack_chords`, which is what makes a cross-port
//! ghost's `slide` intent press the right buttons on each port).
//!
//! Matcher semantics (contract §2): a step is SATISFIED at frame i when its
//! `dirs` are held at frame i and every `press` class's full chord is down
//! AT FRAME i — simultaneously, in that single frame. NO trailing "recently
//! pressed" window: the game reads button state per frame, so simultaneity
//! is the rule, not a lookback. (A press-class onset that lands late still
//! satisfies the chord from the moment it overlaps the other classes still
//! being held — a human chording three buttons rarely lands them on one
//! exact frame, and that overlap IS the simultaneity, not a tolerance grant.)
//! A macro COMPLETES on the rising edge of its final step's satisfaction
//! (satisfied now, not satisfied last frame) — one input is one move, so a
//! chord held for 50 frames fires once, not once per frame. After firing,
//! the macro re-arms only once the final step stops being satisfied
//! (release), not after a fixed frame offset — holding the buttons is one
//! slide, not a slide every N frames. Between steps of a multi-step motion,
//! at most [`MAX_GAP`] frames may elapse (unchanged). `frames` is the
//! executor's hold length; the matcher matches human taps of any length
//! (≥1 frame) instead of enforcing it.
//!
//! §10 extends this with two amendments (Mileena/Reptile MK2 audit):
//!
//! - **Release-triggered and charged steps.** A step's KIND is Normal
//!   (`dirs`/`press`, the original shape), Hold (`hold` + `min_frames`: the
//!   chord must stay down `min_frames` CONTINUOUS frames before the step is
//!   satisfied; a release before that FAILS the whole macro, not just the
//!   step), or Release (`release`: satisfied on the FALLING edge of that
//!   chord). "Completion fires on the edge the FINAL step names" — a macro
//!   ending in a Release step completes on a release, not a press. A
//!   `while_held` chord on any step is an extra AND-condition (like `press`)
//!   that must also be down for that step to satisfy — the step-scoped
//!   stand-in for nesting a hold across other steps (Reptile's Invisibility:
//!   Block held across `U U D`).
//! - **Side-swapping moves.** `back`/`forward` resolve against the facing
//!   LIVE at the moment a macro's first step satisfies, PINNED for the rest
//!   of that attempt (matcher and executor both) — a mid-macro crossup
//!   (Mileena's Teleport Kick) must not silently reinterpret earlier or
//!   later steps against a different side.

use crate::profile::{GameProfile, StepSpec};

/// Max frames between one step's completion and the next step's onset.
pub const MAX_GAP: u64 = 12;
/// Neutral frames the executor inserts between steps so the game (and our
/// own matcher) sees distinct taps — well inside `MAX_GAP`.
const STEP_GAP: u8 = 2;

/// RETRO joypad direction bits (RETRO_DEVICE_ID order).
const BIT_UP: usize = 4;
const BIT_DOWN: usize = 5;
const BIT_LEFT: usize = 6;
const BIT_RIGHT: usize = 7;

/// Semantic direction space — the closed set profiles encode in. `Back` /
/// `Forward` resolve against live side = sign(opp.x − me.x); both matcher
/// and executor use the same resolution (contract §2).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dir {
    Back,
    Forward,
    Up,
    Down,
}

impl Dir {
    pub fn parse(s: &str) -> Option<Dir> {
        Some(match s {
            "back" => Dir::Back,
            "forward" => Dir::Forward,
            "up" => Dir::Up,
            "down" => Dir::Down,
            _ => return None,
        })
    }

    /// Physical RETRO bit for this dir given where the opponent stands.
    fn bit(self, opponent_right: bool) -> usize {
        match self {
            Dir::Up => BIT_UP,
            Dir::Down => BIT_DOWN,
            Dir::Forward => if opponent_right { BIT_RIGHT } else { BIT_LEFT },
            Dir::Back => if opponent_right { BIT_LEFT } else { BIT_RIGHT },
        }
    }
}

/// A step's KIND (§10.1) — mutually exclusive, decides which edge the step
/// fires on and which compiled chord list `held_now` reads.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum StepKind {
    /// `dirs`/`press` down now (the original §2 shape).
    Normal,
    /// `hold` down continuously for `min_frames` frames.
    Hold,
    /// `release` chord down last frame, not down now (falling edge).
    Release,
}

/// One compiled step: semantic dirs + one physical chord mask per press/
/// hold/release CLASS (a class counts as "down" only when its whole chord
/// is down). `while_held` is an extra chord ANDed into satisfaction
/// regardless of kind — the step-scoped stand-in for a hold spanning other
/// steps (§10.1).
#[derive(Clone, Debug)]
struct Step {
    dirs: Vec<Dir>,
    press: Vec<u16>,
    hold: Vec<u16>,
    release: Vec<u16>,
    while_held: Vec<u16>,
    kind: StepKind,
    min_frames: u64,
    frames: u8,
}

/// A macro compiled against one port's `attack_chords` — ready for both the
/// matcher and the executor.
#[derive(Clone, Debug)]
pub struct CompiledMacro {
    pub name: String,
    steps: Vec<Step>,
}

/// Compile a profile encoding. Errors name the offending piece — callers
/// skip the macro with a warning rather than aborting (an unencodable move
/// simply isn't offered, the §2 omission rule).
pub fn compile(name: &str, steps: &[StepSpec], p: &GameProfile) -> Result<CompiledMacro, String> {
    if steps.is_empty() {
        return Err(format!("macro '{name}' has no steps"));
    }
    let compile_classes = |classes: &[String]| -> Result<Vec<u16>, String> {
        classes
            .iter()
            .map(|class| {
                let chord = p
                    .port
                    .attack_chords
                    .get(class)
                    .filter(|c| !c.is_empty())
                    .ok_or_else(|| format!("macro '{name}': class '{class}' has no chord"))?;
                let mut mask = 0u16;
                for b in chord {
                    mask |= 1
                        << crate::profile::retro_button_bit(b)
                            .ok_or_else(|| format!("macro '{name}': unknown button '{b}'"))?;
                }
                Ok(mask)
            })
            .collect::<Result<Vec<u16>, String>>()
    };
    let compiled = steps
        .iter()
        .map(|s| {
            let dirs = s
                .dirs
                .iter()
                .map(|d| Dir::parse(d).ok_or_else(|| format!("macro '{name}': unknown dir '{d}'")))
                .collect::<Result<Vec<_>, _>>()?;
            let press = compile_classes(&s.press)?;
            let hold = compile_classes(&s.hold)?;
            let release = compile_classes(&s.release)?;
            let while_held = compile_classes(&s.while_held)?;
            if dirs.is_empty() && press.is_empty() && hold.is_empty() && release.is_empty() {
                return Err(format!("macro '{name}': empty step"));
            }
            let kind_count =
                [!press.is_empty(), !hold.is_empty(), !release.is_empty()].iter().filter(|k| **k).count();
            if kind_count > 1 {
                return Err(format!("macro '{name}': step mixes press/hold/release — pick one"));
            }
            let kind = if !hold.is_empty() {
                StepKind::Hold
            } else if !release.is_empty() {
                StepKind::Release
            } else {
                StepKind::Normal
            };
            let min_frames = if kind == StepKind::Hold {
                let mf = s.min_frames.filter(|mf| *mf > 0).ok_or_else(|| {
                    format!("macro '{name}': hold step needs a positive min_frames")
                })?;
                mf as u64
            } else {
                0
            };
            Ok(Step { dirs, press, hold, release, while_held, kind, min_frames, frames: s.frames.max(1) })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(CompiledMacro { name: name.to_string(), steps: compiled })
}

// ── the matcher ─────────────────────────────────────────────────────────────

struct MState {
    /// Next step to satisfy. Equal to `steps.len()` means the macro just
    /// completed and is in COOLDOWN: waiting for its final step to release
    /// before re-arming (rising-edge completion, contract §2).
    step: usize,
    /// Completion frame of the previous step (or the last reset) — onsets
    /// must land strictly after it, so a continuous hold can't re-fire.
    activation: u64,
    /// Onset of the CURRENT continuous hold while `step` points at a Hold
    /// step (§10.1); 0 means "not currently holding". Cleared on advance,
    /// reset, or a release-before-`min_frames` failure.
    hold_onset: u64,
    /// Facing pinned at the frame this attempt's first step satisfied
    /// (§10.2) — `None` while still on step 0 (nothing to pin yet), which
    /// is when `back`/`forward` fall back to the LIVE facing passed to
    /// `feed` this frame.
    pinned_facing: Option<bool>,
}

impl MState {
    fn fresh(now: u64) -> MState {
        MState { step: 0, activation: now, hold_onset: 0, pinned_facing: None }
    }
}

/// Streaming recognizer for ONE player's input stream. Feed the 12-bit mask
/// plus live side each frame; completed macro names come back on their
/// completion frame. All in-flight macros run independent state machines.
pub struct Matcher {
    macros: Vec<CompiledMacro>,
    states: Vec<MState>,
    /// 1-based so "onset 0" can mean "never" under strict comparisons.
    frame: u64,
    prev_mask: u16,
    /// Last up→down edge per PHYSICAL button (RETRO bit index). Facing is
    /// resolved to a physical bit at query time (live for an unpinned step
    /// 0, pinned per-macro otherwise, §10.2) rather than baked into a
    /// semantic-space onset table, so the same onset data serves whichever
    /// facing a given macro's attempt is currently pinned to.
    bit_onset: [u64; 12],
}

impl Matcher {
    pub fn new(macros: Vec<CompiledMacro>) -> Self {
        let states = macros.iter().map(|_| MState::fresh(0)).collect();
        Matcher { macros, states, frame: 0, prev_mask: 0, bit_onset: [0; 12] }
    }

    /// Advance one frame. Returns the names completed THIS frame (deduped —
    /// two characters sharing a move name report it once).
    pub fn feed(&mut self, mask: u16, opponent_right: bool) -> Vec<&str> {
        self.frame += 1;
        let now = self.frame;
        // Captured BEFORE this frame's bit_onset/prev_mask update — a
        // Release step's falling edge (§10.1) is "chord fully down in
        // old_mask, not fully down in mask".
        let old_mask = self.prev_mask;
        for i in 0..12 {
            if mask & (1 << i) != 0 && old_mask & (1 << i) == 0 {
                self.bit_onset[i] = now;
            }
        }
        self.prev_mask = mask;

        let mut done: Vec<&str> = Vec::new();
        for (m, st) in self.macros.iter().zip(self.states.iter_mut()) {
            // facing to resolve THIS step's dirs with: pinned once this
            // attempt has one (§10.2), else the live facing passed in.
            let facing_for = |pin: Option<bool>| -> bool { pin.unwrap_or(opponent_right) };
            let dirs_held = |dirs: &[Dir], facing: bool| -> bool {
                dirs.iter().all(|d| mask & (1 << d.bit(facing)) != 0)
            };
            let chord_down = |chords: &[u16], m: u16| -> bool {
                chords.iter().all(|c| m & c == *c)
            };
            // Raw §2/§10.1 satisfaction, dispatched by step KIND — one
            // frame, no memory (Release aside, which needs exactly one
            // frame of look-back to see its own falling edge). This is
            // also the release test the cooldown branch below uses.
            let held_now = |step: &Step, facing: bool| -> bool {
                if !dirs_held(&step.dirs, facing) || !chord_down(&step.while_held, mask) {
                    return false;
                }
                match step.kind {
                    StepKind::Normal => chord_down(&step.press, mask),
                    StepKind::Hold => chord_down(&step.hold, mask),
                    StepKind::Release => {
                        chord_down(&step.release, old_mask) && !chord_down(&step.release, mask)
                    }
                }
            };
            // Step-advance satisfaction: `held_now` PLUS the freshness/gap
            // bookkeeping that keeps multi-step motions distinct from a
            // continuous hold (F,F needs a second forward TAP, not F held).
            // Dir onsets gate this for dir-only steps and for non-first
            // press steps; a first press step only needs its dirs held —
            // that is what lets "hold back, slide, keep holding back, slide
            // again" re-fire on the chord alone. Press itself carries no
            // onset/tolerance bookkeeping any more (§2: simultaneity, not a
            // trailing window) — see `held_now`. (Hold-kind steps never
            // reach this function — see the dedicated branch below.)
            let sat = |step: &Step, activation: u64, first: bool, facing: bool| -> bool {
                if !held_now(step, facing) {
                    return false;
                }
                if step.press.is_empty() || !first {
                    let mut onset_max = 0u64;
                    for d in &step.dirs {
                        let o = self.bit_onset[d.bit(facing)];
                        if o <= activation {
                            return false; // stale hold, not a fresh tap
                        }
                        onset_max = onset_max.max(o);
                    }
                    return onset_max <= activation + MAX_GAP;
                }
                true
            };

            if st.step == m.steps.len() {
                // Cooldown: this macro just completed. Re-arm only once its
                // final step releases — holding the chord is ONE input, so
                // it must not multiply-count completions (contract §2).
                let facing = facing_for(st.pinned_facing);
                if !held_now(&m.steps[m.steps.len() - 1], facing) {
                    *st = MState::fresh(now);
                }
                continue;
            }

            let cur = &m.steps[st.step];

            if cur.kind == StepKind::Hold {
                // §10.1: satisfied only once held `min_frames` CONTINUOUS
                // frames; a release before that FAILS the whole macro (not
                // a soft reset-and-maybe-restart like the generic path
                // below — parking short of the threshold is not progress).
                let facing = facing_for(st.pinned_facing);
                if held_now(cur, facing) {
                    if st.hold_onset == 0 {
                        st.hold_onset = now;
                    }
                    if now - st.hold_onset + 1 >= cur.min_frames {
                        if st.step == 0 {
                            st.pinned_facing = Some(facing);
                        }
                        st.activation = now;
                        st.step += 1;
                        st.hold_onset = 0;
                        if st.step == m.steps.len() && !done.contains(&m.name.as_str()) {
                            done.push(&m.name);
                        }
                    }
                } else if st.hold_onset != 0 {
                    *st = MState::fresh(now);
                }
                continue;
            }

            let first = st.step == 0;
            let facing = facing_for(st.pinned_facing);
            if sat(cur, st.activation, first, facing) {
                if first {
                    st.pinned_facing = Some(facing);
                }
                st.activation = now;
                st.step += 1;
                if st.step == m.steps.len() {
                    if !done.contains(&m.name.as_str()) {
                        done.push(&m.name);
                    }
                    // Stays at step == len (cooldown) — see above, NOT an
                    // immediate reset, so a held final chord can't re-fire
                    // next frame just because it's still down.
                }
            } else if st.step > 0 {
                // A fresh step-0 satisfaction mid-macro restarts the window
                // (B … stray … B+HP still reads as B, B+HP), re-pinning
                // facing to THIS restart's live side (§10.2 — a restart is a
                // new attempt). Skipped when step 0 is Hold-kind: its
                // `held_now` is a continuous "still down" condition, not a
                // discrete tap, so re-checking it here would spuriously
                // "restart" every single frame the chord stays down.
                // Otherwise a blown gap resets to neutral.
                if m.steps[0].kind != StepKind::Hold && sat(&m.steps[0], now - 1, true, opponent_right) {
                    *st = MState {
                        step: 1,
                        activation: now,
                        hold_onset: 0,
                        pinned_facing: Some(opponent_right),
                    };
                } else if now > st.activation + MAX_GAP {
                    *st = MState::fresh(now);
                }
            }
        }
        done
    }
}

// ── the executor ────────────────────────────────────────────────────────────

/// Plays a compiled macro back one frame at a time: each step's dirs+
/// presses (or +hold / +while_held, by KIND — §10.1) held for its frames
/// ([`Step::min_frames`] for a Hold step, `frames` otherwise), [`STEP_GAP`]
/// neutral frames between steps (so double-taps register). Facing is PINNED
/// on the first `next()` call and reused for the macro's whole duration
/// (§10.2) — a mid-macro side switch (Mileena's Teleport Kick) must not
/// silently flip what "back"/"forward" mean partway through playback.
pub struct MacroExec {
    m: CompiledMacro,
    step: usize,
    hold_left: u32,
    gap_left: u8,
    pinned_facing: Option<bool>,
}

impl MacroExec {
    pub fn new(m: CompiledMacro) -> Self {
        let hold = Self::hold_len(&m.steps[0]);
        MacroExec { m, step: 0, hold_left: hold, gap_left: 0, pinned_facing: None }
    }

    fn hold_len(step: &Step) -> u32 {
        if step.kind == StepKind::Hold {
            step.min_frames as u32
        } else {
            step.frames as u32
        }
    }

    /// The next frame's held-button set; `None` once the macro has finished.
    /// Facing is captured from THIS call the first time it's invoked, then
    /// pinned (§10.2) — later calls' `opponent_right` is ignored.
    pub fn next(&mut self, opponent_right: bool) -> Option<[bool; 12]> {
        if self.step >= self.m.steps.len() {
            return None;
        }
        let facing = *self.pinned_facing.get_or_insert(opponent_right);
        if self.gap_left > 0 {
            self.gap_left -= 1;
            return Some([false; 12]);
        }
        let st = &self.m.steps[self.step];
        let mut bits = [false; 12];
        for d in &st.dirs {
            bits[d.bit(facing)] = true;
        }
        // Release-kind steps press nothing (they exist to let go of
        // `release`'s chord — that's achieved simply by NOT setting its
        // bits here, since `bits` is rebuilt from scratch every frame);
        // Hold-kind steps hold `hold`'s chord instead of `press`'s.
        let press_like: &[u16] = match st.kind {
            StepKind::Normal => &st.press,
            StepKind::Hold => &st.hold,
            StepKind::Release => &[],
        };
        for chord in press_like.iter().chain(&st.while_held) {
            for i in 0..12 {
                if chord & (1 << i) != 0 {
                    bits[i] = true;
                }
            }
        }
        self.hold_left -= 1;
        if self.hold_left == 0 {
            self.step += 1;
            if self.step < self.m.steps.len() {
                self.gap_left = STEP_GAP;
                self.hold_left = Self::hold_len(&self.m.steps[self.step]);
            }
        }
        Some(bits)
    }

    /// Cancel this macro early. Returns the all-neutral mask the caller
    /// MUST inject on abort (§10.1 consequence): parking on a held chord
    /// (e.g. mid-Sai-Throw HP, or Invisibility's held Block) silently arms
    /// a charge move, so an aborted executor cannot simply stop being
    /// polled — it must actively hand back a release.
    pub fn abort(&mut self) -> [bool; 12] {
        self.step = self.m.steps.len();
        [false; 12]
    }
}

// ── block-punish option pool ────────────────────────────────────────────────

/// One entry in the block-punish dummy's weighted pool (contract §6).
/// `{throw: true}` is deferred until throw RE lands.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum PunishOption {
    /// A named special from the dummy character's family∩port intersection.
    Move(String),
    /// A base attack class (single chord press).
    Attack(String),
    /// Keep guarding for N frames.
    ContinueBlock(u16),
}

impl PunishOption {
    pub fn label(&self) -> String {
        match self {
            PunishOption::Move(n) => n.clone(),
            PunishOption::Attack(c) => c.clone(),
            PunishOption::ContinueBlock(_) => "Continue Block".into(),
        }
    }
}

/// Weighted sample from a pool — xorshift64* over the caller's seed (no rand
/// dep; the trigger MUST never be deterministic, the survey's #1 finding,
/// so callers mix wall-clock entropy into the seed).
pub fn weighted_pick<T>(pool: &[(T, u8)], seed: u64) -> Option<&T> {
    let total: u32 = pool.iter().map(|(_, w)| *w as u32).sum();
    if total == 0 {
        return None;
    }
    let mut x = seed | 1;
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    let mut roll = (x.wrapping_mul(0x2545F4914F6CDD1D) % total as u64) as u32;
    for (opt, w) in pool {
        let w = *w as u32;
        if roll < w {
            return Some(opt);
        }
        roll -= w;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::GameProfile;
    use std::path::PathBuf;
    use std::env;

    /// A minimal two-port MK2-shaped pair of profiles built in a temp dir —
    /// the tests must not read library/mk2/genesis.profile.json (edited
    /// concurrently); the cross-port point only needs the two chord tables.
    fn test_profile(tag: &str, chords: &str) -> GameProfile {
        let base = std::env::temp_dir().join("rustretro_tests").join(format!(
            "macros_{tag}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(
            base.join("family.json"),
            r#"{"family":"t","roster":[{"id":9,"name":"reptile"}],"move_classes":[],
                "attack_classes":["None","HP","LP","HK","LK","Block"],
                "moves":{"reptile":[{"name":"slide","tags":["special","low"]},
                                    {"name":"acid_spit","tags":["special","projectile"]}]}}"#,
        )
        .unwrap();
        let port = format!(
            r#"{{"family":"t","port":"{tag}","core":{{"library_name":"","provenance_game":"t","provenance_core":"t"}},
                "memory":{{"blocks":{{"block1":"0x0","block2":"0x0","stride":"0x0"}},"fighter_fields":[],"globals":{{}}}},
                "gate":[],"enforcement":{{"health_max":255,"refill_below":1,"timer_hold":[0,0],"credits_target":0,"credits_min":0}},
                "calibration":{{}},"attack_chords":{chords}}}"#
        );
        std::fs::write(base.join(format!("{tag}.profile.json")), port).unwrap();
        GameProfile::load(&PathBuf::from(base.join(tag))).expect("test profile loads")
    }

    fn arcade() -> GameProfile {
        // mk2 arcade chords: HP=y LP=b HK=x LK=a Block=l
        test_profile("arcade", r#"{"HP":["y"],"LP":["b"],"HK":["x"],"LK":["a"],"Block":["l"]}"#)
    }
    fn genesis() -> GameProfile {
        // mk2 genesis chords: HK=l LK=r HP=y LP=b Block=a
        test_profile("genesis", r#"{"HP":["y"],"LP":["b"],"HK":["l"],"LK":["r"],"Block":["a"]}"#)
    }

    fn spec(json: &str) -> Vec<crate::profile::StepSpec> {
        serde_json::from_str(json).unwrap()
    }

    fn slide_arcade(p: &GameProfile) -> CompiledMacro {
        compile("slide", &spec(r#"[{"dirs":["back"],"press":["LK","LP"],"frames":4}]"#), p).unwrap()
    }

    const B_LEFT: u16 = 1 << 6;
    const B_RIGHT: u16 = 1 << 7;
    const B_B: u16 = 1; // LP arcade chord
    const B_A: u16 = 1 << 8; // LK arcade chord
    const B_Y: u16 = 1 << 1; // HP chord

    #[test]
    fn slide_chord_matches_regardless_of_press_stagger() {
        let p = arcade();
        // Opponent to the right → back = Left. LK lands 2 frames after LP.
        let mut m = Matcher::new(vec![slide_arcade(&p)]);
        assert!(m.feed(B_LEFT | B_B, true).is_empty());
        assert!(m.feed(B_LEFT | B_B, true).is_empty());
        assert_eq!(m.feed(B_LEFT | B_B | B_A, true), vec!["slide"]);
        // Held chord must not re-fire on the following frames.
        assert!(m.feed(B_LEFT | B_B | B_A, true).is_empty());

        // 5-frame stagger: LP is held from frame 1; LK lands 5 frames later
        // while LP is still down. The game reads state, not history — once
        // both are simultaneously held (whenever that happens to land), the
        // chord is satisfied. It fires exactly once (rising edge), then
        // holding the completed chord produces no further completions.
        let mut m = Matcher::new(vec![slide_arcade(&p)]);
        assert!(m.feed(B_LEFT | B_B, true).is_empty());
        for _ in 0..4 {
            assert!(m.feed(B_LEFT | B_B, true).is_empty());
        }
        assert_eq!(m.feed(B_LEFT | B_B | B_A, true), vec!["slide"]);
        for _ in 0..5 {
            assert!(m.feed(B_LEFT | B_B | B_A, true).is_empty());
        }
    }

    #[test]
    fn slide_chord_never_matches_when_presses_dont_overlap() {
        let p = arcade();
        // LP pressed and released BEFORE LK is ever pressed: the two class
        // chords never share a frame, so the chord never completes — this
        // is one input each, not a chord (contract §2: simultaneity, not a
        // trailing "recently pressed" window).
        let mut m = Matcher::new(vec![slide_arcade(&p)]);
        assert!(m.feed(B_LEFT | B_B, true).is_empty());
        assert!(m.feed(B_LEFT, true).is_empty()); // LP released
        assert!(m.feed(B_LEFT | B_A, true).is_empty()); // LK pressed alone
        assert!(m.feed(B_LEFT | B_A, true).is_empty());
    }

    #[test]
    fn slide_requires_the_back_direction() {
        let p = arcade();
        let mut m = Matcher::new(vec![slide_arcade(&p)]);
        // Same chord with FORWARD held (opp right → Right) is not a slide.
        assert!(m.feed(B_RIGHT | B_B | B_A, true).is_empty());
        // And bare chord without any dir is not either.
        let mut m = Matcher::new(vec![slide_arcade(&p)]);
        assert!(m.feed(B_B | B_A, true).is_empty());
    }

    #[test]
    fn motion_sequence_matches_across_gaps_within_max_gap_only() {
        let p = arcade();
        let acid = |p: &GameProfile| {
            compile(
                "acid_spit",
                &spec(r#"[{"dirs":["forward"],"frames":3},{"dirs":["forward"],"press":["HP"],"frames":3}]"#),
                p,
            )
            .unwrap()
        };
        // F tap, 6-frame gap, F+HP → match on the chord frame.
        let mut m = Matcher::new(vec![acid(&p)]);
        assert!(m.feed(B_RIGHT, true).is_empty()); // step 0 completes here
        assert!(m.feed(B_RIGHT, true).is_empty());
        for _ in 0..6 {
            assert!(m.feed(0, true).is_empty());
        }
        assert_eq!(m.feed(B_RIGHT | B_Y, true), vec!["acid_spit"]);

        // Same shape but the gap blows MAX_GAP → no match (machine reset).
        let mut m = Matcher::new(vec![acid(&p)]);
        assert!(m.feed(B_RIGHT, true).is_empty());
        for _ in 0..13 {
            assert!(m.feed(0, true).is_empty());
        }
        assert!(m.feed(B_RIGHT | B_Y, true).is_empty());

        // A continuous forward HOLD is one step, not two: no match ever.
        let mut m = Matcher::new(vec![acid(&p)]);
        for _ in 0..10 {
            assert!(m.feed(B_RIGHT, true).is_empty());
        }
        assert!(m.feed(B_RIGHT | B_Y, true).is_empty(), "held F is not F,F");
    }

    #[test]
    fn facing_flip_mid_macro_pins_the_starting_side_10_2() {
        // §10.2 supersedes the old "live facing every frame" rule: a macro
        // pins facing at the frame its first step satisfies and keeps that
        // pin for the rest of the attempt (a crossup must not silently
        // reinterpret "forward" partway through a motion).
        let p = arcade();
        let ff = compile(
            "ff",
            &spec(r#"[{"dirs":["forward"],"frames":3},{"dirs":["forward"],"frames":3}]"#),
            &p,
        )
        .unwrap();
        let mut m = Matcher::new(vec![ff]);
        // First forward tap with the opponent on the RIGHT — pins forward =
        // physical Right for this attempt.
        assert!(m.feed(B_RIGHT, true).is_empty());
        assert!(m.feed(0, true).is_empty());
        // Side switch reported at the SAME frame the player presses physical
        // Right again: under the OLD live rule this would be "back" (since
        // live-forward is now Left) and would not complete; PINNED, it is
        // still forward (the pin, not the live report, decides) — completes.
        assert_eq!(m.feed(B_RIGHT, false), vec!["ff"]);

        // Counter-case: after the same flip, pressing what is now LIVE
        // forward (physical Left) must NOT satisfy the PINNED macro's
        // second step — that is exactly the bug §10.2 closes (a stale
        // semantic label must not let a different physical button sneak in
        // as if it were the same motion).
        let ff2 = compile(
            "ff",
            &spec(r#"[{"dirs":["forward"],"frames":3},{"dirs":["forward"],"frames":3}]"#),
            &p,
        )
        .unwrap();
        let mut m = Matcher::new(vec![ff2]);
        assert!(m.feed(B_RIGHT, true).is_empty()); // forward (opp right) -> pin=right
        assert!(m.feed(0, false).is_empty()); // release, side now reported left
        // Pressing physical Left here does NOT satisfy the pinned attempt's
        // second step (pin says forward=Right); it can only be read as a
        // fresh, independent start (§2's existing "stray tap restarts the
        // window" rule) — either way, this single frame does not complete.
        assert!(m.feed(B_LEFT, false).is_empty());
    }

    #[test]
    fn executor_emits_port_correct_slide_masks() {
        let arc = arcade();
        let gen = genesis();
        let arc_slide = slide_arcade(&arc);
        let gen_slide =
            compile("slide", &spec(r#"[{"dirs":["back"],"press":["LK","HK"],"frames":4}]"#), &gen)
                .unwrap();

        let run = |m: CompiledMacro| -> Vec<u16> {
            let mut ex = MacroExec::new(m);
            let mut out = Vec::new();
            while let Some(bits) = ex.next(true) {
                out.push(crate::record::pack_mask(&bits));
            }
            out
        };
        // Arcade: back(Left)=0x40 + LK(a)=0x100 + LP(b)=0x1, held 4 frames.
        assert_eq!(run(arc_slide), vec![0x141; 4]);
        // Genesis: back(Left)=0x40 + LK(r)=0x800 + HK(l)=0x400.
        assert_eq!(run(gen_slide), vec![0xC40; 4]);
        // The cross-port point: same move name, different masks per port.
        assert_ne!(0x141, 0xC40);
    }

    #[test]
    fn executor_pins_facing_from_the_first_call_10_2() {
        // §10.2 supersedes "dirs re-resolved every frame": the executor
        // pins facing on its first `next()` call and reuses it for the
        // whole macro, so a mid-macro side switch does NOT flip later
        // steps' physical buttons.
        let p = arcade();
        let acid = compile(
            "acid_spit",
            &spec(r#"[{"dirs":["forward"],"frames":3},{"dirs":["forward"],"press":["HP"],"frames":3}]"#),
            &p,
        )
        .unwrap();
        let mut ex = MacroExec::new(acid);
        let mut masks = Vec::new();
        let mut i = 0;
        // Report the opponent flipping to the left mid-macro (frames 0-3
        // right, rest left) — the pin from frame 0 must win throughout.
        while let Some(bits) = ex.next(i < 4) {
            masks.push(crate::record::pack_mask(&bits));
            i += 1;
        }
        // 3×F(right), 2×neutral gap, 3×F(right)+HP — the LATER "left" report
        // is ignored; forward stays physical Right for this whole macro.
        assert_eq!(masks, vec![0x80, 0x80, 0x80, 0, 0, 0x82, 0x82, 0x82]);
    }

    #[test]
    fn executor_plays_back_hold_for_min_frames_then_releases_10_1() {
        let p = arcade();
        let sai_throw = compile(
            "sai_throw",
            &spec(r#"[{"hold":["HP"],"min_frames":5},{"release":["HP"]}]"#),
            &p,
        )
        .unwrap();
        let mut ex = MacroExec::new(sai_throw);
        let mut masks = Vec::new();
        while let Some(bits) = ex.next(true) {
            masks.push(crate::record::pack_mask(&bits));
        }
        // 5 frames holding HP (0x2), STEP_GAP=2 neutral frames, then the
        // release step's own `frames` (default 3) neutral frames — the
        // release step presses nothing, which IS the release.
        assert_eq!(masks, vec![0x2, 0x2, 0x2, 0x2, 0x2, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn executor_round_trips_a_hold_release_macro_through_the_matcher_10_1() {
        let p = arcade();
        let make = || {
            compile("sai_throw", &spec(r#"[{"hold":["HP"],"min_frames":5},{"release":["HP"]}]"#), &p)
                .unwrap()
        };
        let mut ex = MacroExec::new(make());
        let mut m = Matcher::new(vec![make()]);
        let mut seen = Vec::new();
        while let Some(bits) = ex.next(true) {
            for n in m.feed(crate::record::pack_mask(&bits), true) {
                seen.push(n.to_string());
            }
        }
        assert_eq!(seen, vec!["sai_throw"], "the executor's own release must satisfy its own matcher");
    }

    #[test]
    fn executor_abort_returns_neutral_and_stops_holding_10_1() {
        // §10.1 consequence: aborting mid-Hold must release the chord, not
        // park on it (a parked chord silently arms a charge move).
        let p = arcade();
        let sai_throw = compile(
            "sai_throw",
            &spec(r#"[{"hold":["HP"],"min_frames":150},{"release":["HP"]}]"#),
            &p,
        )
        .unwrap();
        let mut ex = MacroExec::new(sai_throw);
        // A few frames into the hold, HP is genuinely being held.
        for _ in 0..5 {
            let bits = ex.next(true).expect("still mid-hold");
            assert!(bits[1], "HP bit should be held during the hold step"); // HP=y=bit1
        }
        let abort_bits = ex.abort();
        assert_eq!(abort_bits, [false; 12], "abort must hand back an all-neutral release");
        assert!(ex.next(true).is_none(), "an aborted executor is done, not paused");
    }

    #[test]
    fn executor_output_round_trips_through_the_matcher() {
        let p = arcade();
        let mut ex = MacroExec::new(slide_arcade(&p));
        let mut m = Matcher::new(vec![slide_arcade(&p)]);
        let mut seen = Vec::new();
        while let Some(bits) = ex.next(true) {
            for n in m.feed(crate::record::pack_mask(&bits), true) {
                seen.push(n.to_string());
            }
        }
        assert_eq!(seen, vec!["slide"], "the pair must agree on its own output");
    }

    #[test]
    fn compile_rejects_unknown_class_and_empty_step() {
        let p = arcade();
        assert!(compile("x", &spec(r#"[{"press":["Fireball"]}]"#), &p)
            .unwrap_err()
            .contains("no chord"));
        assert!(compile("x", &spec(r#"[{}]"#), &p).unwrap_err().contains("empty step"));
        assert!(compile("x", &[], &p).unwrap_err().contains("no steps"));
    }

    #[test]
    fn compile_rejects_bad_hold_release_shapes_10_1() {
        let p = arcade();
        // hold without min_frames.
        assert!(compile("x", &spec(r#"[{"hold":["HP"]}]"#), &p)
            .unwrap_err()
            .contains("min_frames"));
        // press + hold in the same step (kind ambiguity).
        assert!(compile("x", &spec(r#"[{"press":["HP"],"hold":["LP"],"min_frames":5}]"#), &p)
            .unwrap_err()
            .contains("mixes"));
        // press + release in the same step.
        assert!(compile("x", &spec(r#"[{"press":["HP"],"release":["LP"]}]"#), &p)
            .unwrap_err()
            .contains("mixes"));
        // bare while_held with no dirs/press/hold/release is still an
        // empty step (it names no discrete action of its own).
        assert!(compile("x", &spec(r#"[{"while_held":["Block"]}]"#), &p)
            .unwrap_err()
            .contains("empty step"));
        // a well-formed hold step compiles fine.
        assert!(compile("x", &spec(r#"[{"hold":["HP"],"min_frames":150}]"#), &p).is_ok());
    }

    #[test]
    fn weighted_pick_respects_weights_and_zero_total() {
        let pool = vec![(PunishOption::Move("slide".into()), 3u8), (PunishOption::ContinueBlock(30), 1)];
        let mut counts = [0u32; 2];
        for seed in 0..4000u64 {
            match weighted_pick(&pool, seed ^ (seed << 17)) {
                Some(PunishOption::Move(_)) => counts[0] += 1,
                Some(PunishOption::ContinueBlock(_)) => counts[1] += 1,
                _ => panic!("pool is non-empty"),
            }
        }
        // ~3:1 split; loose bounds — this asserts "weighted", not "exact".
        assert!(counts[0] > counts[1] * 2, "{counts:?}");
        assert!(counts[1] > 400, "{counts:?}");
        let empty: Vec<(PunishOption, u8)> = vec![(PunishOption::ContinueBlock(1), 0)];
        assert!(weighted_pick(&empty, 7).is_none(), "all-zero weights pick nothing");
    }

    #[test]
    fn golden_fixture_parity() {
        // Load the golden fixture from shadow/train/tests/fixtures/matcher_golden.json
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let fixture_path = manifest_dir
            .join("shadow/train/tests/fixtures/matcher_golden.json");
        let fixture_json = std::fs::read_to_string(&fixture_path)
            .expect("golden fixture file exists");

        #[derive(serde::Deserialize, Debug)]
        struct GoldenCase {
            name: String,
            description: Option<String>,
            #[serde(rename = "macro")]
            macro_steps: Vec<serde_json::Value>,
            facing: serde_json::Value,
            frames: Vec<String>,
            expected: Vec<GoldenExpectation>,
        }

        #[derive(serde::Deserialize, Debug)]
        struct GoldenExpectation {
            frame: usize,
            #[serde(rename = "move")]
            move_name: String,
        }

        let cases: Vec<GoldenCase> = serde_json::from_str(&fixture_json)
            .expect("golden fixture parses as JSON");
        assert!(!cases.is_empty(), "golden fixture must contain cases to be a gate at all");
        let mut exercised = 0usize;

        // Create the test profile (arcade-shaped with MK2 chords).
        let profile = arcade();

        for case in cases {
            exercised += 1;
            // Determine the move name from expected completions or case name.
            let move_name = if !case.expected.is_empty() {
                case.expected[0].move_name.clone()
            } else if case.name.contains("slide") {
                "slide".to_string()
            } else if case.name.contains("acid_spit") {
                "acid_spit".to_string()
            } else {
                case.name.clone()
            };

            // Parse the facing field (string or per-frame array).
            let num_frames = case.frames.len();
            let sides: Vec<bool> = if let serde_json::Value::String(s) = &case.facing {
                vec![s == "right"; num_frames]
            } else if let serde_json::Value::Array(arr) = &case.facing {
                arr.iter()
                    .map(|v| {
                        v.as_str().expect("facing element is string") == "right"
                    })
                    .collect()
            } else {
                panic!("facing must be string or array");
            };

            // Parse frames from hex strings.
            let masks: Vec<u16> = case
                .frames
                .iter()
                .map(|s| u16::from_str_radix(s.trim_start_matches("0x"), 16)
                    .expect("frame is valid hex"))
                .collect();

            // Compile the macro steps using the determined move name.
            let macro_spec = serde_json::to_string(&case.macro_steps)
                .expect("macro_steps serialize");
            let steps: Vec<crate::profile::StepSpec> =
                serde_json::from_str(&macro_spec)
                    .expect("macro steps parse");

            let compiled = compile(&move_name, &steps, &profile)
                .expect(&format!("macro '{}' compiles", move_name));

            // Run the matcher frame-by-frame.
            // Note: Rust's Matcher uses 1-based frame numbering; our fixture uses 0-based indexing.
            // When feed() is called at array index i, Rust's internal frame counter is i+1.
            // But we want to report completions using the array index for consistency with Python.
            let mut matcher = Matcher::new(vec![compiled]);
            let mut actual_events: Vec<(usize, String)> = Vec::new();
            for (frame_idx, (mask, opponent_right)) in
                masks.iter().zip(sides.iter()).enumerate()
            {
                let completions = matcher.feed(*mask, *opponent_right);
                for _move_name in completions {
                    // Report completion at the 0-indexed array position where it occurred
                    actual_events.push((frame_idx, move_name.clone()));
                }
            }

            // Convert expected to the same format.
            let expected_events: Vec<(usize, String)> = case
                .expected
                .iter()
                .map(|e| (e.frame, e.move_name.clone()))
                .collect();

            // This IS the gate: fail on any mismatch, naming the case so a
            // regression is traceable straight back to its fixture entry.
            assert_eq!(
                actual_events, expected_events,
                "golden case '{}' diverged: expected {:?}, got {:?}",
                case.name, expected_events, actual_events,
            );
        }

        assert!(
            exercised >= 9,
            "golden fixture parity only exercised {exercised} cases — fixture shrank?"
        );
    }

    /// The §10 twin of `golden_fixture_parity`: same shared-fixture design
    /// (`shadow/train/tests/fixtures/macro_ext_golden.json`, also run by
    /// Python's `tests/test_macro_ext_parity.py`), covering the NEW step
    /// kinds (hold/release/while_held, §10.1) and pinned-facing resolution
    /// across a side swap (§10.2). Each case names its own `move_name`
    /// explicitly (no inference needed, unlike the §2 fixture).
    #[test]
    fn golden_ext_fixture_parity() {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let fixture_path = manifest_dir.join("shadow/train/tests/fixtures/macro_ext_golden.json");
        let fixture_json =
            std::fs::read_to_string(&fixture_path).expect("§10 golden fixture file exists");

        #[derive(serde::Deserialize, Debug)]
        struct ExtCase {
            name: String,
            move_name: String,
            #[serde(rename = "macro")]
            macro_steps: Vec<serde_json::Value>,
            facing: serde_json::Value,
            frames: Vec<String>,
            expected: Vec<ExtExpectation>,
        }
        #[derive(serde::Deserialize, Debug)]
        struct ExtExpectation {
            frame: usize,
            #[serde(rename = "move")]
            move_name: String,
        }

        let cases: Vec<ExtCase> =
            serde_json::from_str(&fixture_json).expect("§10 golden fixture parses as JSON");
        assert!(cases.len() >= 6, "§10 golden fixture must contain its cases to be a gate at all");

        let profile = arcade();
        for case in cases {
            let masks: Vec<u16> = case
                .frames
                .iter()
                .map(|s| u16::from_str_radix(s.trim_start_matches("0x"), 16).expect("frame is valid hex"))
                .collect();
            let sides: Vec<bool> = match &case.facing {
                serde_json::Value::String(s) => vec![s == "right"; masks.len()],
                serde_json::Value::Array(arr) => {
                    arr.iter().map(|v| v.as_str().expect("facing is string") == "right").collect()
                }
                _ => panic!("facing must be string or array"),
            };

            let steps: Vec<crate::profile::StepSpec> =
                serde_json::from_str(&serde_json::to_string(&case.macro_steps).unwrap())
                    .expect("§10 macro steps parse");
            let compiled = compile(&case.move_name, &steps, &profile)
                .unwrap_or_else(|e| panic!("§10 case '{}': macro compiles: {e}", case.name));

            let mut matcher = Matcher::new(vec![compiled]);
            let mut actual: Vec<(usize, String)> = Vec::new();
            for (i, (mask, side)) in masks.iter().zip(sides.iter()).enumerate() {
                for name in matcher.feed(*mask, *side) {
                    actual.push((i, name.to_string()));
                }
            }
            let expected: Vec<(usize, String)> =
                case.expected.iter().map(|e| (e.frame, e.move_name.clone())).collect();

            assert_eq!(
                actual, expected,
                "§10 golden case '{}' diverged: expected {:?}, got {:?}",
                case.name, expected, actual,
            );
        }
    }
}
