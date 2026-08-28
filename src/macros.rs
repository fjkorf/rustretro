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
//! Matcher semantics (contract §2): within a step, every press class must be
//! fully down simultaneously for ≥1 frame while the step's dirs are held —
//! class onsets may straggle up to [`CHORD_TOLERANCE`] frames; between steps
//! at most [`MAX_GAP`] frames. A completed step is consumed: re-firing needs
//! fresh rising edges, so a held chord matches once, not every frame.
//! `frames` is the executor's hold length; the matcher matches human taps of
//! any length (≥1 frame) instead of enforcing it.

use crate::profile::{GameProfile, StepSpec};

/// Max frames between one step's completion and the next step's onset.
pub const MAX_GAP: u64 = 12;
/// Max spread (frames) between the press-class onsets inside one step.
pub const CHORD_TOLERANCE: u64 = 3;
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
    /// Next step to satisfy.
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
            // A step is satisfied NOW when its dirs are held, its classes are
            // fully down within CHORD_TOLERANCE of each other, and the
            // relevant onsets are FRESH (strictly after `activation`, i.e.
            // after the previous step completed). Dir onsets count for
            // dir-only steps and for non-first press steps (F,F+HP needs a
            // second forward TAP); a first press step only needs its dirs
            // held — that is what lets "hold back, slide, keep holding back,
            // slide again" re-fire on the chord alone.
            let sat = |step: &Step, activation: u64, first: bool| -> bool {
                if !step.dirs.iter().all(|d| self.prev_dir[d.idx()]) {
                    return false;
                }
                let mut onset_max = 0u64;
                if step.press.is_empty() || !first {
                    for d in &step.dirs {
                        let o = self.dir_onset[d.idx()];
                        if o <= activation {
                            return false; // stale hold, not a fresh tap
                        }
                        onset_max = onset_max.max(o);
                    }
                }
                if !step.press.is_empty() {
                    let mut lo = u64::MAX;
                    let mut hi = 0u64;
                    for chord in &step.press {
                        if mask & chord != *chord {
                            return false; // class not fully down
                        }
                        let o = (0..12)
                            .filter(|i| chord & (1 << i) != 0)
                            .map(|i| self.bit_onset[i])
                            .max()
                            .unwrap_or(0);
                        lo = lo.min(o);
                        hi = hi.max(o);
                    }
                    if hi - lo > CHORD_TOLERANCE {
                        return false; // presses straggled too far apart
                    }
                    if hi <= activation {
                        return false; // consumed chord must not re-fire
                    }
                    onset_max = onset_max.max(hi);
                }
                first || onset_max <= activation + MAX_GAP
            };

            if sat(&m.steps[st.step], st.activation, st.step == 0) {
                st.activation = now;
                st.step += 1;
                if st.step == m.steps.len() {
                    if !done.contains(&m.name.as_str()) {
                        done.push(&m.name);
                    }
                    *st = MState { step: 0, activation: now };
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
    fn slide_chord_matches_with_two_frame_stagger_not_five() {
        let p = arcade();
        // Opponent to the right → back = Left. LK lands 2 frames after LP.
        let mut m = Matcher::new(vec![slide_arcade(&p)]);
        assert!(m.feed(B_LEFT | B_B, true).is_empty());
        assert!(m.feed(B_LEFT | B_B, true).is_empty());
        assert_eq!(m.feed(B_LEFT | B_B | B_A, true), vec!["slide"]);
        // Held chord must not re-fire on the following frames.
        assert!(m.feed(B_LEFT | B_B | B_A, true).is_empty());

        // 5-frame stagger: never a match, even though both end up held.
        let mut m = Matcher::new(vec![slide_arcade(&p)]);
        assert!(m.feed(B_LEFT | B_B, true).is_empty());
        for _ in 0..4 {
            assert!(m.feed(B_LEFT | B_B, true).is_empty());
        }
        for _ in 0..6 {
            assert!(m.feed(B_LEFT | B_B | B_A, true).is_empty());
        }
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
}
