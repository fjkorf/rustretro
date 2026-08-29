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

    /// Index into the matcher's semantic-space tracking arrays.
    fn idx(self) -> usize {
        match self {
            Dir::Back => 0,
            Dir::Forward => 1,
            Dir::Up => 2,
            Dir::Down => 3,
        }
    }
}

/// One compiled step: semantic dirs + one physical chord mask per press
/// CLASS (a class counts as "down" only when its whole chord is down).
#[derive(Clone, Debug)]
struct Step {
    dirs: Vec<Dir>,
    press: Vec<u16>,
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
    let compiled = steps
        .iter()
        .map(|s| {
            let dirs = s
                .dirs
                .iter()
                .map(|d| Dir::parse(d).ok_or_else(|| format!("macro '{name}': unknown dir '{d}'")))
                .collect::<Result<Vec<_>, _>>()?;
            let press = s
                .press
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
                            << crate::profile::retro_button_bit(b).ok_or_else(|| {
                                format!("macro '{name}': unknown button '{b}'")
                            })?;
                    }
                    Ok(mask)
                })
                .collect::<Result<Vec<u16>, String>>()?;
            if dirs.is_empty() && press.is_empty() {
                return Err(format!("macro '{name}': empty step"));
            }
            Ok(Step { dirs, press, frames: s.frames.max(1) })
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
    /// Last up→down edge per physical button.
    bit_onset: [u64; 12],
    /// Held/onset tracking in SEMANTIC space (Back/Forward/Up/Down) — a
    /// facing flip mid-hold reads as a release+press of the semantic dir,
    /// which is exactly the physical truth of the input.
    prev_dir: [bool; 4],
    dir_onset: [u64; 4],
}

impl Matcher {
    pub fn new(macros: Vec<CompiledMacro>) -> Self {
        let states = macros.iter().map(|_| MState { step: 0, activation: 0 }).collect();
        Matcher {
            macros,
            states,
            frame: 0,
            prev_mask: 0,
            bit_onset: [0; 12],
            prev_dir: [false; 4],
            dir_onset: [0; 4],
        }
    }

    /// Advance one frame. Returns the names completed THIS frame (deduped —
    /// two characters sharing a move name report it once).
    pub fn feed(&mut self, mask: u16, opponent_right: bool) -> Vec<&str> {
        self.frame += 1;
        let now = self.frame;
        for i in 0..12 {
            if mask & (1 << i) != 0 && self.prev_mask & (1 << i) == 0 {
                self.bit_onset[i] = now;
            }
        }
        for d in [Dir::Back, Dir::Forward, Dir::Up, Dir::Down] {
            let held = mask & (1 << d.bit(opponent_right)) != 0;
            let i = d.idx();
            if held && !self.prev_dir[i] {
                self.dir_onset[i] = now;
            }
            self.prev_dir[i] = held;
        }
        self.prev_mask = mask;

        let mut done: Vec<&str> = Vec::new();
        for (m, st) in self.macros.iter().zip(self.states.iter_mut()) {
            // Raw §2 satisfaction: dirs held now, every press class's full
            // chord down now — one frame, no memory. This is also the
            // release test the cooldown branch below uses.
            let held_now = |step: &Step| -> bool {
                step.dirs.iter().all(|d| self.prev_dir[d.idx()])
                    && step.press.iter().all(|chord| mask & chord == *chord)
            };
            // Step-advance satisfaction: `held_now` PLUS the freshness/gap
            // bookkeeping that keeps multi-step motions distinct from a
            // continuous hold (F,F needs a second forward TAP, not F held).
            // Dir onsets gate this for dir-only steps and for non-first
            // press steps; a first press step only needs its dirs held —
            // that is what lets "hold back, slide, keep holding back, slide
            // again" re-fire on the chord alone. Press itself carries no
            // onset/tolerance bookkeeping any more (§2: simultaneity, not a
            // trailing window) — see `held_now`.
            let sat = |step: &Step, activation: u64, first: bool| -> bool {
                if !held_now(step) {
                    return false;
                }
                if step.press.is_empty() || !first {
                    let mut onset_max = 0u64;
                    for d in &step.dirs {
                        let o = self.dir_onset[d.idx()];
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
                if !held_now(&m.steps[m.steps.len() - 1]) {
                    *st = MState { step: 0, activation: now };
                }
                continue;
            }

            if sat(&m.steps[st.step], st.activation, st.step == 0) {
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
                // (B … stray … B+HP still reads as B, B+HP); otherwise a
                // blown gap resets to neutral.
                if sat(&m.steps[0], now - 1, true) {
                    *st = MState { step: 1, activation: now };
                } else if now > st.activation + MAX_GAP {
                    *st = MState { step: 0, activation: now };
                }
            }
        }
        done
    }
}

// ── the executor ────────────────────────────────────────────────────────────

/// Plays a compiled macro back one frame at a time: each step's dirs+presses
/// held for its `frames`, [`STEP_GAP`] neutral frames between steps (so
/// double-taps register), dirs re-resolved against live facing EVERY frame —
/// a mid-macro side switch keeps "back" meaning away (contract §5).
pub struct MacroExec {
    m: CompiledMacro,
    step: usize,
    hold_left: u8,
    gap_left: u8,
}

impl MacroExec {
    pub fn new(m: CompiledMacro) -> Self {
        let hold = m.steps[0].frames;
        MacroExec { m, step: 0, hold_left: hold, gap_left: 0 }
    }

    /// The next frame's held-button set; `None` once the macro has finished.
    pub fn next(&mut self, opponent_right: bool) -> Option<[bool; 12]> {
        if self.step >= self.m.steps.len() {
            return None;
        }
        if self.gap_left > 0 {
            self.gap_left -= 1;
            return Some([false; 12]);
        }
        let st = &self.m.steps[self.step];
        let mut bits = [false; 12];
        for d in &st.dirs {
            bits[d.bit(opponent_right)] = true;
        }
        for chord in &st.press {
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
                self.hold_left = self.m.steps[self.step].frames;
            }
        }
        Some(bits)
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
    fn facing_flip_mid_macro_tracks_semantic_dirs() {
        let p = arcade();
        let ff = compile(
            "ff",
            &spec(r#"[{"dirs":["forward"],"frames":3},{"dirs":["forward"],"frames":3}]"#),
            &p,
        )
        .unwrap();
        let mut m = Matcher::new(vec![ff]);
        // First forward tap with the opponent on the RIGHT (physical Right).
        assert!(m.feed(B_RIGHT, true).is_empty());
        assert!(m.feed(0, true).is_empty());
        // Side switch: opponent now LEFT — forward is physical Left. The
        // player taps Left and the motion still completes.
        assert_eq!(m.feed(B_LEFT, false), vec!["ff"]);

        // Counter-case: holding Left through a flip becomes a fresh semantic
        // BACK press — a back-charge matcher would see an onset. For the
        // forward pair it contributes nothing:
        let ff2 = compile(
            "ff",
            &spec(r#"[{"dirs":["forward"],"frames":3},{"dirs":["forward"],"frames":3}]"#),
            &p,
        )
        .unwrap();
        let mut m = Matcher::new(vec![ff2]);
        assert!(m.feed(B_RIGHT, true).is_empty()); // forward (opp right)
        assert!(m.feed(B_RIGHT, false).is_empty()); // flip: same button is now BACK
        assert!(m.feed(B_RIGHT, false).is_empty()); // still held: no fresh forward
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
    fn executor_resolves_facing_per_frame_and_gaps_between_steps() {
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
        // Flip the opponent's side mid-macro: frames 0-3 right, rest left.
        while let Some(bits) = ex.next(i < 4) {
            masks.push(crate::record::pack_mask(&bits));
            i += 1;
        }
        // 3×F(right), 2×neutral gap — flip lands inside the gap — 3×F(left)+HP.
        assert_eq!(masks, vec![0x80, 0x80, 0x80, 0, 0, 0x42, 0x42, 0x42]);
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
}
