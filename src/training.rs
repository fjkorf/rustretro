//! Training mode (shadow PLAN Wave 2b): an infinite, resettable practice
//! fight for demonstration recording.
//!
//! Enabled with `--training`/F5. Each emulated frame, [`tick`] applies every
//! enforcement the loaded profile can support — **per-feature, not
//! all-or-nothing**, so a partially-mapped game (MK2) gets health refill and
//! the dummy while its unmapped enforcements (timer hold, credits, position
//! reset) decline individually:
//! - **credits topped up** so Start always works (`credits` global),
//! - **round timer held** — no timeouts (`round_timer` global),
//! - **health refill**: below the threshold every mapped health byte is
//!   rewritten to max — fighter-block health/health2 plus any per-player HUD
//!   accumulator globals (`p1_health_hud`/`p2_health_hud`; MK2 damages all
//!   four independently) — so damage/hitstun stay visible but nobody is ever
//!   KO'd (toggle with F3),
//! - **dummy control**: a preset drives controller port 1 (F1 cycles
//!   Free / Stand / Crouch / Jump / Block) — Block holds away from the other
//!   fighter using live X positions (fighter-field `x` or `p1_x`/`p2_x`
//!   globals),
//! - **position reset** (F2, needs X source + explicit `positions`) and
//!   **finish round now** (F4, needs `round_state`) one-shots.
//!
//! The in-fight gate is the profile's `gate` condition list — the SAME gate
//! the recorder and Lua `game.controllable()` evaluate (via `crate::gate::eval_gate`,
//! shared with `lua_engine`). A profile with no gate list (a stub) has no training
//! at all; [`available`]/[`features`] tell the panel what to offer.
//!
//! All writes go through `DebugState::write_addr`: bus-window addresses queue
//! onto the live 68k bus via the Sek write queue; direct-pointer regions
//! (FBNeo System RAM fallback) are written in place. `freeze` does NOT land
//! on the latter (mk2.md) — per-frame re-assertion here is the workaround.

use crate::debug::{DebugState, DummyMode, GuardMode};
use crate::profile::GameProfile;

/// RETRO joypad direction bits used by the guard hold (RETRO_DEVICE_ID order).
const BIT_LEFT: usize = 6;
const BIT_RIGHT: usize = 7;

/// The dummy occupies fighter block 2: it is injected on controller port 1,
/// and port 1 drives block 2 (asurabld.md verified this live; MK2's `p2_*`
/// globals are the same pairing). Deriving it from live X instead — as this
/// used to — mis-attributes the dummy the moment the fighters cross up.
const DUMMY_BLOCK: u8 = 2;

/// Frames the reactive guard keeps holding away after the commitment signal
/// clears. The `attacking` FIELD is exact, so this only covers our own
/// one-frame read/inject latency.
const GUARD_FIELD_TAIL: u64 = 4;

/// Frames the reactive guard holds away after an attack PRESS when the only
/// commitment source is the opponent's input mask (§9.2 fallback): the press
/// is an instant, so this stands in for startup + active.
const GUARD_INPUT_TAIL: u64 = 20;

/// Which training features the loaded profile supports — the panel uses this
/// to disable individual controls with honest hints instead of hiding the
/// whole mode behind one all-or-nothing message.
pub struct Features {
    pub refill: bool,
    pub timer_hold: bool,
    pub credits: bool,
    pub position_reset: bool,
    pub finish_round: bool,
    pub block_dummy: bool,
    /// BlockPunish needs a guard AND a trigger: a contact signal
    /// (`hitstun_sources`/`contact_signal`, MACRO_ACTIONS §6) for button
    /// families, or the attack-commitment window for back-hold families
    /// (§9.3 — blocked contact is undetectable there).
    pub block_punish: bool,
    /// `GuardMode::AfterFirstHit` needs a contact signal to know a hit landed.
    pub guard_after_hit: bool,
}

impl Features {
    /// Feature labels that are NOT mapped for this game (panel hint line).
    pub fn missing(&self) -> Vec<&'static str> {
        let mut m = Vec::new();
        if !self.refill {
            m.push("health refill");
        }
        if !self.timer_hold {
            m.push("timer hold");
        }
        if !self.credits {
            m.push("credits top-up");
        }
        if !self.position_reset {
            m.push("position reset");
        }
        if !self.finish_round {
            m.push("finish round");
        }
        if !self.block_dummy {
            m.push("Block dummy");
        }
        if !self.block_punish {
            m.push("Block-punish dummy");
        }
        if !self.guard_after_hit {
            m.push("guard After First Hit");
        }
        m
    }
}

/// One fighter's refill writes: `check` is the authoritative struct health
/// consulted against the threshold; `addrs` is every byte rewritten to max
/// (struct health, health2 if mapped, per-player HUD accumulator if mapped).
struct RefillSide {
    check: u32,
    addrs: Vec<u32>,
}

struct Refill {
    sides: [RefillSide; 2],
    max: u8,
    below: u8,
}

/// Resolved absolute X addresses for the two fighters (block field or globals).
#[derive(Clone, Copy)]
struct XPair {
    p1: u32,
    p2: u32,
}

/// Where fighter field `x` (and, where mapped, `y`) comes from — resolved
/// once per `resolve()` call (cheap; `resolve` itself is called fresh every
/// tick, see [`Resolved`]'s doc).
#[derive(Clone, Copy)]
enum XSource {
    /// A stable absolute address per block: the plain `off` form, or the
    /// legacy per-block `globals` form.
    Fixed(XPair),
    /// Pointer-resolved (`via: "object_ptr"`, docs/frames.md §5): the object
    /// pool slot moves every frame, so there is no address to cache — each
    /// read dereferences fresh through [`GameProfile::object_ptr_field`] and
    /// yields ABSENT (never 0) when the pointer is invalid or the char-id
    /// cross-check fails THIS frame (RECORDER_V3 law).
    ObjectPtr,
}

struct Reset {
    x: XPair,
    left: u16,
    right: u16,
    /// (block1 y, block2 y, ground) — only when the profile maps Y.
    y: Option<(u32, u32, u16)>,
}

/// Values resolved from the loaded `GameProfile` once per `tick` call — kept
/// as plain locals rather than a cached/static struct so the profile stays
/// hot-swappable-in-theory and the hot path stays simple (small `Vec`/`BTreeMap`
/// lookups on a handful of fields, once per emulated frame — cheap).
struct Resolved {
    little: bool,
    refill: Option<Refill>,
    timer: Option<(u32, [u8; 2])>,
    credits: Option<(u32, u8, u8)>,
    reset: Option<Reset>,
    finish: Option<u32>,
    x_pair: Option<XSource>,
    /// The family's guard POLICY (MACRO_ACTIONS §9.1) — how the dummy blocks.
    guard: Guard,
    /// The BlockPunish trigger source (MACRO_ACTIONS §6): per-block hitstun
    /// globals where mapped (asurabld), else the port's global contact signal
    /// (MK2 arcade's hit_counter). None → the mode degrades to plain Block.
    contact: Option<Contact>,
    /// Cooldown window: the signal must be quiet this long to re-arm.
    hitstun_window: u64,
}

/// The guard policy, resolved from `family.block.style` (MACRO_ACTIONS §9.1).
///
/// `button` is positionally inert — the chord can be held forever. `back_hold`
/// is NOT: a continuously-guarding asurabld dummy walked itself 165 → 286
/// units into the corner in ~1 s (live-measured, §9). So back-hold families
/// guard REACTIVELY: neutral by default, away-direction only inside the guard
/// window.
enum Guard {
    /// Hold this chord for as long as we are guarding (MK).
    Button([bool; 12]),
    /// Reactive away-hold. `range` = max |opp.x − me.x| for the window to open
    /// (None = unmapped, no distance gate); `commit` = the attack-commitment
    /// source; `tail` = frames the window stays open after it clears.
    BackHold { range: Option<i32>, commit: Commit, tail: u64 },
    /// The profile maps neither a block chord nor a usable window — the dummy
    /// stands still and the panel greys the guard modes (house degradation).
    Inert,
}

/// Attack-commitment sources in MACRO_ACTIONS §9.2 preference order (we SHIP
/// the field path where the port maps one; the input mask is the zero-RE
/// fallback that works on any family).
enum Commit {
    /// Per-fighter "committing an attack" flag, (block1 addr, block2 addr) —
    /// asurabld's `attacking` (+0x6F): 0 at rest, 1 for the full live duration
    /// of any attack. Read the OPPONENT's.
    Field(u32, u32),
    /// The opponent port's live input mask: any attack-class chord fully down.
    InputMask(Vec<u16>),
}

/// One frame of guard-policy output.
struct GuardOut {
    /// What the dummy holds this frame (empty = stand neutral).
    bits: [bool; 12],
    /// The opponent is committed to an attack inside `guard_range` — the §9.3
    /// punish event. Independent of the guard MODE: it describes the opponent,
    /// not our choice. Always true for button families (no window).
    commit: bool,
    /// Rising edge of `commit` — one opportunity, one trigger.
    commit_edge: bool,
    /// The mode took this opportunity and we are asserting the guard.
    guarding: bool,
    /// Where the opponent stands, for macro direction resolution.
    opp_right: bool,
}

/// Resolved contact-signal addresses. Change = contact (hit OR blocked hit).
enum Contact {
    /// Per-block globals (`hitstun_sources`): read the DUMMY's block.
    PerBlock(u32, u32),
    /// One global counter for both players (`contact_signal`).
    Global(u32),
}

fn resolve(p: &GameProfile) -> Option<Resolved> {
    // No gate list = no way to know when we're in a fight — training as a
    // whole must no-op rather than enforce on menus (same class as the
    // QA-found Record crash: stub profiles refuse softly).
    if p.port.gate.is_empty() {
        return None;
    }
    let g = |name: &str| p.global(name);
    let field = |name: &str| p.field_off(name).map(|(off, _)| off);
    let e = &p.port.enforcement;

    let x_pair = if p.field_is_object_ptr("x") {
        // Pointer-resolved (MK2 arcade, docs/frames.md §5): no fixed address
        // — every read dereferences the object pool fresh (see `read_via_x`).
        Some(XSource::ObjectPtr)
    } else {
        field("x")
            .map(|off| XSource::Fixed(XPair { p1: p.block1() + off, p2: p.block2() + off }))
            .or_else(|| Some(XSource::Fixed(XPair { p1: g("p1_x")?, p2: g("p2_x")? })))
    };

    let refill = field("health").map(|off| {
        let side = |base: u32, hud: &str| {
            let mut addrs = vec![base + off];
            if let Some(h2) = field("health2") {
                addrs.push(base + h2);
            }
            if let Some(a) = g(hud) {
                addrs.push(a);
            }
            RefillSide { check: base + off, addrs }
        };
        Refill {
            sides: [side(p.block1(), "p1_health_hud"), side(p.block2(), "p2_health_hud")],
            max: e.health_max,
            below: e.refill_below,
        }
    });

    let reset = (|| {
        // Position reset needs a WRITABLE fixed address — a pointer-resolved
        // x (docs/frames.md §5) has none yet (a follow-up would need to
        // dereference-then-write through the same live decode), so it
        // declines here exactly like an unmapped field.
        let XSource::Fixed(x) = x_pair? else { return None };
        // Explicit positions required — no silent asurabld-shaped defaults
        // teleporting an unmapped game to nonsense coordinates.
        let left = *p.port.positions.get("round_start_x_left")? as u16;
        let right = *p.port.positions.get("round_start_x_right")? as u16;
        let y = (|| {
            let off = field("y")?;
            let ground = *p.port.positions.get("round_start_y")? as u16;
            Some((p.block1() + off, p.block2() + off, ground))
        })();
        Some(Reset { x, left, right, y })
    })();

    // ── the guard policy (§9.1) ────────────────────────────────────────────
    let guard = if p.family.block.style == "button" {
        p.family
            .block
            .class
            .as_deref()
            .and_then(|class| {
                // An empty chord (block button not yet verified for this port)
                // must not resolve into a hold-nothing "block" — fall through.
                let chord = p.port.attack_chords.get(class).filter(|c| !c.is_empty())?;
                let mut bits = [false; 12];
                for name in chord {
                    bits[crate::profile::retro_button_bit(name)? as usize] = true;
                }
                Some(Guard::Button(bits))
            })
            .unwrap_or(Guard::Inert)
    } else {
        // Reactive guard: needs X (for the away direction AND the distance
        // gate) plus an attack-commitment source.
        (|| {
            x_pair?;
            let commit = field("attacking")
                .map(|off| (Commit::Field(p.block1() + off, p.block2() + off), GUARD_FIELD_TAIL))
                .or_else(|| {
                    // §9.2 fallback: the opponent's live input mask. The block
                    // class itself is never an attack (button families only).
                    let block_class = p.family.block.class.as_deref();
                    let masks: Vec<u16> = p
                        .port
                        .attack_chords
                        .iter()
                        .filter(|(class, buttons)| {
                            !buttons.is_empty() && Some(class.as_str()) != block_class
                        })
                        .filter_map(|(_, buttons)| {
                            let mut m = 0u16;
                            for b in buttons {
                                m |= 1 << crate::profile::retro_button_bit(b)?;
                            }
                            Some(m)
                        })
                        .collect();
                    (!masks.is_empty()).then(|| (Commit::InputMask(masks), GUARD_INPUT_TAIL))
                })?;
            Some(Guard::BackHold {
                range: p
                    .port
                    .block
                    .as_ref()
                    .and_then(|b| b.guard_range)
                    .map(|r| r as i32),
                commit: commit.0,
                tail: commit.1,
            })
        })()
        .unwrap_or(Guard::Inert)
    };

    // `contact_signal` FIRST: it is the purpose-built "was struck" signal.
    // hitstun_sources is a health delta — blind to zero-chip blocked hits
    // (the user-reported "punishes some hits but not others") and disturbed
    // by refill writes — so it is only the fallback.
    let contact = p
        .port
        .contact_signal
        .as_ref()
        .and_then(|cs| match (&cs.field, &cs.global) {
            (Some(f), _) => Some(Contact::PerBlock(
                p.field_addr(1, f)?.0,
                p.field_addr(2, f)?.0,
            )),
            (None, Some(gl)) => g(gl).map(Contact::Global),
            _ => None,
        })
        .or_else(|| {
            p.port.hitstun_sources.as_ref().and_then(|hs| {
                Some(Contact::PerBlock(g(hs.get("block1")?)?, g(hs.get("block2")?)?))
            })
        });

    Some(Resolved {
        little: p.port.memory.endianness == "little",
        refill,
        timer: g("round_timer").map(|a| (a, e.timer_hold)),
        credits: g("credits").map(|a| (a, e.credits_target, e.credits_min)),
        reset,
        finish: g("round_state"),
        x_pair,
        guard,
        contact,
        hitstun_window: p.calibration("HITSTUN_RECENT_FRAMES").unwrap_or(20.0) as u64,
    })
}

fn rd8(ds: &DebugState, addr: u32) -> u8 {
    ds.read_addr(addr as usize, 1).unwrap_or(0) as u8
}

fn rd16(ds: &DebugState, addr: u32, little: bool) -> u16 {
    let v = ds.read_addr(addr as usize, 2).unwrap_or(0) as u16;
    if little { v } else { v.swap_bytes() }
}

fn wr8(ds: &mut DebugState, addr: u32, v: u8) {
    let _ = ds.write_addr(addr as usize, 1, v as u32);
}

fn wr16(ds: &mut DebugState, addr: u32, v: u16, little: bool) {
    // write_addr takes little-endian value bytes to ascending addresses; swap
    // for big-endian guests (68k) so the guest reads `v`.
    let v = if little { v } else { v.swap_bytes() };
    let _ = ds.write_addr(addr as usize, 2, v as u32);
}

/// Endian-correct a raw `read_addr` value of `size` bytes (1/2/4) — `rd8`/
/// `rd16` inlined this for their own fixed widths; `object_ptr_field`'s
/// reader closure needs it generically (the pointer word is 4 bytes).
fn endian_fix(v: u32, size: u8, little: bool) -> u32 {
    if little {
        return v;
    }
    match size {
        2 => (v as u16).swap_bytes() as u32,
        4 => v.swap_bytes(),
        _ => v,
    }
}

/// Live-read a pointer-resolved fighter field (`x`/`y`, docs/frames.md §5)
/// for one block. `None` — ABSENT, never 0 — when the object pointer is
/// invalid or the char-id cross-check fails THIS frame (RECORDER_V3 law).
fn read_object_ptr(ds: &DebugState, p: &GameProfile, little: bool, block: u8, name: &str) -> Option<i32> {
    p.object_ptr_field(block, name, |addr, size| {
        endian_fix(ds.read_addr(addr as usize, size as usize).unwrap_or(0), size, little)
    })
    .map(|v| v as i32)
}

/// Live-read fighter field `x` for `block`, honoring the resolved source
/// (fixed address, or the pointer-resolved form).
fn read_via_x(ds: &DebugState, p: &GameProfile, little: bool, xs: XSource, block: u8) -> Option<i32> {
    match xs {
        XSource::Fixed(xp) => {
            let addr = if block == 1 { xp.p1 } else { xp.p2 };
            Some(rd16(ds, addr, little) as i32)
        }
        XSource::ObjectPtr => read_object_ptr(ds, p, little, block, "x"),
    }
}

/// Whether the loaded profile supports training at all (has an in-fight
/// gate). Per-feature detail comes from [`features`].
pub fn available() -> bool {
    resolve(crate::profile::current()).is_some()
}

/// Per-feature availability for the loaded profile — `None` when training is
/// unavailable entirely (no gate list).
pub fn features() -> Option<Features> {
    features_of(crate::profile::current())
}

fn features_of(p: &GameProfile) -> Option<Features> {
    let r = resolve(p)?;
    let block_dummy = !matches!(r.guard, Guard::Inert);
    // Back-hold families trigger on the commitment window, not on contact
    // (§9.3: blocked contact is undetectable there), so a resolved reactive
    // guard IS the trigger.
    let has_trigger = r.contact.is_some() || matches!(r.guard, Guard::BackHold { .. });
    Some(Features {
        refill: r.refill.is_some(),
        timer_hold: r.timer.is_some(),
        credits: r.credits.is_some(),
        position_reset: r.reset.is_some(),
        finish_round: r.finish.is_some(),
        block_dummy,
        block_punish: block_dummy && has_trigger,
        guard_after_hit: r.contact.is_some(),
    })
}

/// Wall-clock entropy, so nothing the dummy samples is deterministic (§6's
/// number-one survey finding).
fn entropy() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0)
}

/// Poll the profile's contact signal for the DUMMY (it is the victim of the
/// punish drill): `(changed this frame, current value)`. Also stamps the
/// quiet-window bookkeeping the punish cooldown and the guard modes read.
fn poll_contact(ds: &mut DebugState, r: &Resolved, frame: u64) -> (bool, u8) {
    let Some(contact) = &r.contact else {
        return (false, 0);
    };
    let addr = match contact {
        Contact::Global(a) => *a,
        Contact::PerBlock(b1, b2) => {
            if DUMMY_BLOCK == 1 {
                *b1
            } else {
                *b2
            }
        }
    };
    let cur = rd8(ds, addr);
    let changed = ds.training.punish_prev_signal.is_some_and(|prev| prev != cur);
    ds.training.punish_prev_signal = Some(cur);
    if changed {
        ds.training.punish_last_change = frame;
    }
    (changed, cur)
}

/// One frame of the guard policy (MACRO_ACTIONS §9): decide whether there is a
/// guard opportunity (style-dependent), whether the MODE takes it, and what to
/// hold. Shared by `DummyMode::Block` and `DummyMode::BlockPunish` — one
/// policy, so both dummies behave identically about spacing.
fn guard_frame(
    ds: &mut DebugState,
    p: &GameProfile,
    r: &Resolved,
    frame: u64,
    contact_changed: bool,
) -> GuardOut {
    // Live geometry (both styles want `opp_right` for macro playback). A
    // pointer-resolved x (docs/frames.md §5) can transiently read ABSENT
    // (invalid pointer / stale char-id this exact frame) — `None` here, same
    // as an unmapped x, never a synthesized 0.
    let opp_block = if DUMMY_BLOCK == 1 { 2 } else { 1 };
    let geom = r.x_pair.and_then(|xs| {
        let me = read_via_x(ds, p, r.little, xs, DUMMY_BLOCK)?;
        let opp = read_via_x(ds, p, r.little, xs, opp_block)?;
        Some((me, opp))
    });
    let opp_right = geom.map(|(me, opp)| opp > me).unwrap_or(false);

    // ── (a) Is there a guard opportunity this frame? ──────────────────────
    let (raw, away) = match &r.guard {
        // A held button is positionally inert, so a button family has no
        // window to open: the opportunity is simply always (today's
        // behaviour, unchanged).
        Guard::Button(_) => (true, None),
        Guard::Inert => (false, None),
        Guard::BackHold { range, commit, tail } => match geom {
            // A fixed-address x always resolves once `x_pair` is Some — this
            // is only reachable for a pointer-resolved x going transiently
            // stale (no back_hold family ships one today; MK2 is `button`
            // style). No window this frame, rather than a crash.
            None => (false, None),
            Some((me, opp)) => {
                // Away = the direction that increases the gap. NEVER Down:
                // down-back blocks nothing on asurabld (0/4, reproduced) —
                // the guard hold is PURE standing back.
                let away = Some(if me >= opp { BIT_RIGHT } else { BIT_LEFT });
                let live = match commit {
                    Commit::Field(b1, b2) => {
                        rd8(ds, if DUMMY_BLOCK == 1 { *b2 } else { *b1 }) != 0
                    }
                    Commit::InputMask(masks) => {
                        let mask = crate::record::pack_mask(&ds.input_state);
                        masks.iter().any(|m| mask & m == *m)
                    }
                };
                if live {
                    ds.training.guard_commit_until = frame + tail;
                }
                let committed = live || frame < ds.training.guard_commit_until;
                // The distance gate is what keeps the dummy from reacting to
                // far whiffs (and from drifting on them).
                let in_range = range.is_none_or(|rg| (me - opp).abs() <= rg);
                (committed && in_range, away)
            }
        },
    };

    // ── (b) String bookkeeping for the modes that need a hit signal ───────
    if contact_changed {
        ds.training.guard_hit_seen = true;
        ds.training.guard_last_hit = frame;
    } else if ds.training.guard_hit_seen
        && frame.saturating_sub(ds.training.guard_last_hit) > r.hitstun_window
    {
        // The string ended: After First Hit re-arms, Random re-rolls.
        ds.training.guard_hit_seen = false;
        ds.training.guard_roll = None;
    }
    // One opportunity = one sticky decision; it expires with the window (for
    // button families, whose window never closes, the string end above is what
    // expires it).
    if !raw {
        ds.training.guard_roll = None;
    }

    // ── (c) The MODE decides whether we take it (§9.4) ────────────────────
    let allow = match ds.training.guard_mode {
        GuardMode::All => true,
        GuardMode::None => false,
        GuardMode::AfterFirstHit => ds.training.guard_hit_seen,
        GuardMode::Random => match ds.training.guard_roll {
            Some(v) => v,
            None if raw => {
                let pct = ds.training.guard_random_pct.0.min(100);
                let seed = frame ^ (entropy() << 13) ^ 0x9E37_79B9;
                let v = crate::macros::weighted_pick(&[(true, pct), (false, 100 - pct)], seed)
                    .copied()
                    .unwrap_or(false);
                ds.training.guard_roll = Some(v);
                v
            }
            None => false,
        },
    };

    let commit_edge = raw && !ds.training.guard_prev_commit;
    ds.training.guard_prev_commit = raw;

    let guarding = raw && allow;
    let mut bits = [false; 12];
    if guarding {
        match &r.guard {
            Guard::Button(chord) => bits = *chord,
            Guard::BackHold { .. } => {
                if let Some(b) = away {
                    bits[b] = true;
                }
            }
            Guard::Inert => {}
        }
    }
    GuardOut { bits, commit: raw, commit_edge, guarding, opp_right }
}

/// Run one training-mode frame. Called from `Frontend::run_frame` after the
/// bus-window refresh (reads see this frame's snapshot; writes drain to the
/// live bus next frame).
pub fn tick(ds: &mut DebugState, frame: u64) {
    tick_with(ds, frame, crate::profile::current());
}

/// [`tick`] against an explicit profile — the testable seam (the process
/// profile is a OnceLock, so per-game tick tests pass their own).
fn tick_with(ds: &mut DebugState, frame: u64, p: &GameProfile) {
    // Input-slot record/playback (task A2) is its own feature, independent
    // of `TrainingConfig::enabled` — it runs even when training mode itself
    // is off. This is the ONE place per real emulated frame that already
    // gets called unconditionally from `Frontend::run_frame` (see `tick`'s
    // doc), which is exactly the frame-exact hook `playback::tick` needs
    // (docs/frames.md §3/§4 determinism — see `playback.rs`'s module doc).
    crate::playback::tick(ds, frame, p);

    if !ds.training.enabled {
        return;
    }
    let Some(r) = resolve(p) else {
        // Stub profile: no in-fight gate mapped — refuse softly, once.
        ds.training.enabled = false;
        ds.log("🎯 Training unavailable: this game's profile has no in-fight gate yet".into());
        eprintln!("[training] unavailable: profile has no gate conditions (stub) — disabled");
        return;
    };
    // Credits top-up, checked once a second: Start must always work.
    if let Some((addr, target, min)) = r.credits {
        if frame % 60 == 0 && rd8(ds, addr) < min {
            wr8(ds, addr, target);
        }
    }
    if !crate::gate::eval_gate(ds, p) {
        // A punish macro already in flight keeps playing through SHORT gate
        // closures — MK2 arcade zeroes its in-fight word at the very contact
        // that triggers the punish (hit-freeze; live-observed 2026-08-28), so
        // stalling here would strand every punish two frames in. A closure
        // longer than the grace is a real round end and drops the macro.
        // Nothing else runs while closed: enforcement stays off menus.
        if ds.training.dummy == DummyMode::BlockPunish {
            // The phase must stay TRUTHFUL while gated: the mode isn't
            // running, so saying "punishing" (the stale trigger label)
            // hides the real reason the dummy is standing still.
            ds.training.punish_phase = "gate closed — not in a fight".to_string();
        }
        if ds.training.punish_exec.is_some() {
            ds.training.punish_gate_grace += 1;
            let mut bits = None;
            if ds.training.punish_gate_grace > PUNISH_GATE_GRACE {
                ds.training.punish_exec = None;
            } else if ds.training.punish_wait > 0 {
                // Ride out blockstun guarding, then release for a clean press.
                ds.training.punish_wait -= 1;
                bits = Some(if ds.training.punish_wait < PUNISH_RELEASE {
                    [false; 12]
                } else {
                    // Same reactive policy as in-gate (x reads are ungated).
                    guard_frame(ds, p, &r, frame, false).bits
                });
            } else {
                let opp_right = guard_frame(ds, p, &r, frame, false).opp_right;
                if let Some(ex) = ds.training.punish_exec.as_mut() {
                    bits = ex.next(opp_right);
                }
                if bits.is_none() {
                    ds.training.punish_exec = None;
                }
            }
            // Playback wins (task A2 §4): if an input-slot playback is
            // actively driving P2 this frame, the punish macro's write is
            // skipped entirely rather than blended with it — see
            // `playback.rs`'s precedence doc.
            if let Some(bits) = bits {
                if !crate::playback::active_on_port(ds, 1) {
                    for (i, on) in bits.iter().enumerate() {
                        ds.injected_input2[i] = if *on { 2 } else { 0 };
                    }
                }
            }
        }
        return;
    }
    ds.training.punish_gate_grace = 0;
    // Hold the round clock.
    if let Some((addr, hold)) = r.timer {
        wr8(ds, addr, hold[0]);
        wr8(ds, addr + 1, hold[1]);
    }
    // Health refill: let damage show, never let anyone die. Every mapped
    // accumulator for the refilled fighter is rewritten (MK2's HUD pair
    // tracks damage independently of the struct byte — mk2.md).
    if ds.training.refill {
        if let Some(rf) = &r.refill {
            let fired: Vec<(usize, u8, Vec<u32>)> = rf
                .sides
                .iter()
                .enumerate()
                .filter_map(|(i, s)| {
                    let h = rd8(ds, s.check);
                    (h < rf.below).then(|| (i, h, s.addrs.clone()))
                })
                .collect();
            for (side, was, addrs) in fired {
                for addr in addrs {
                    wr8(ds, addr, rf.max);
                }
                ds.log(format!("🎯 refill: P{} {was} → {}", side + 1, rf.max));
            }
        }
    }
    // One-shots — each declines with a log line when its map is missing.
    if ds.training.reset_positions {
        ds.training.reset_positions = false;
        match &r.reset {
            Some(rs) => {
                let (x1, x2) = (rd16(ds, rs.x.p1, r.little), rd16(ds, rs.x.p2, r.little));
                let (b1x, b2x) =
                    if x1 <= x2 { (rs.left, rs.right) } else { (rs.right, rs.left) };
                wr16(ds, rs.x.p1, b1x, r.little);
                wr16(ds, rs.x.p2, b2x, r.little);
                if let Some((y1, y2, ground)) = rs.y {
                    wr16(ds, y1, ground, r.little);
                    wr16(ds, y2, ground, r.little);
                }
            }
            None => ds.log("🎯 Position reset: not mapped for this game".into()),
        }
    }
    if ds.training.finish_round {
        ds.training.finish_round = false;
        match r.finish {
            Some(addr) => wr8(ds, addr, 0),
            None => ds.log("🎯 Finish round: not mapped for this game".into()),
        }
    }
    // Dummy preset → port-1 injection (2-frame holds so they bridge to the
    // next GUI fold without latching).
    let dummy_bits: Option<[bool; 12]> = match ds.training.dummy {
        DummyMode::Free => None,
        DummyMode::Stand => Some([false; 12]),
        DummyMode::Crouch => {
            let mut b = [false; 12];
            b[5] = true; // Down
            Some(b)
        }
        DummyMode::Jump => {
            let mut b = [false; 12];
            // Tap Up half a second out of every second → repeated hops.
            b[4] = (frame / 30) % 2 == 0;
            Some(b)
        }
        // Guard per family block style: the chord for button-block families
        // (MK), hold-away for back-hold families (see `guard_hold`).
        DummyMode::Block => {
            let (changed, _) = poll_contact(ds, &r, frame);
            Some(guard_frame(ds, p, &r, frame, changed).bits)
        }
        // Guard, and on each trigger sample the weighted punish pool
        // (MACRO_ACTIONS §6/§9.3). No trigger mapped → plain guarding (the
        // panel greys the mode with the reason).
        DummyMode::BlockPunish => {
            let contact = poll_contact(ds, &r, frame);
            let g = guard_frame(ds, p, &r, frame, contact.0);
            Some(block_punish(ds, frame, p, &r, g, contact))
        }
    };
    // Playback wins over the training dummy (task A2 §4): an input-slot
    // playback actively driving P2 this frame suppresses the dummy's write
    // entirely (never blended via the fold's OR) — see `playback.rs`'s
    // precedence doc and `active_on_port`'s doc for why `started && !done`
    // is the right cutover point (a merely-ARMED `RoundStart` playback
    // leaves the dummy untouched until it actually triggers).
    if let Some(bits) = dummy_bits {
        if !crate::playback::active_on_port(ds, 1) {
            for (i, on) in bits.iter().enumerate() {
                ds.injected_input2[i] = if *on { 2 } else { 0 };
            }
        }
    }
}

/// Frames between the contact trigger and the macro's first input: the
/// dummy keeps guarding through hit-freeze + its own blockstun, then
/// punishes — inputs played into the freeze are eaten by the game
/// (live-observed on MK2 arcade, 2026-08-28). This is
/// [`crate::debug::ReversalTiming`]'s DEFAULT (`Explicit(PUNISH_DELAY)`) —
/// unchanged behaviour on a fresh install: 26 ≈ hit-freeze (~10) + jab
/// blockstun (~14) + slack — a chord played at +16 was still eaten while a
/// motion whose chord lands at +21 came out — live-calibrated on MK2 arcade
/// 2026-08-28. See [`PUNISH_DELAY_FAST`]/[`PUNISH_DELAY_LATE`] for
/// `ReversalTiming::Fast`/`Late`.
pub const PUNISH_DELAY: u64 = 26;

/// `ReversalTiming::Fast`'s floor: one frame below this (+16 — see
/// [`PUNISH_DELAY`]'s note) was live-observed to get eaten by hit-freeze/
/// blockstun, so this is the FASTEST a reversal can start and still
/// actually come out. `Fast` carries no user-supplied frame count — it
/// always resolves to this — so the "first possible frame" mode can never
/// silently pick a value that never fires (unlike `Explicit`, which is an
/// unclamped power-user knob).
pub const PUNISH_DELAY_FAST: u64 = 21;

/// `ReversalTiming::Late`'s value: comfortably later than the fitted
/// [`PUNISH_DELAY`] default (roughly double its slack margin over
/// [`PUNISH_DELAY_FAST`]) while staying well inside `PUNISH_GATE_GRACE`.
/// This is a GLOBAL calibration, not a per-move "last safe frame" — a true
/// per-move value needs the frames.json measurement table (docs/frames.md
/// §6), which does not exist yet (see docs/frames.md §10, "Stated
/// limitations").
pub const PUNISH_DELAY_LATE: u64 = 34;

/// Resolve a [`crate::debug::ReversalTiming`] mode to actual wait-frames for
/// ONE punish event. `seed` is wall-clock entropy (§6: "never deterministic")
/// so `Delay` re-rolls independently on every scheduled punish — pass a seed
/// distinct from whatever seeded the punish-POOL pick made in the same frame.
fn resolve_reversal_delay(timing: crate::debug::ReversalTiming, seed: u64) -> u64 {
    use crate::debug::ReversalTiming::*;
    match timing {
        Fast => PUNISH_DELAY_FAST,
        Late => PUNISH_DELAY_LATE,
        Explicit(frames) => frames,
        Delay { min, max } => {
            let (lo, hi) = if min <= max { (min, max) } else { (max, min) };
            if hi <= lo { lo } else { lo + seed % (hi - lo + 1) }
        }
    }
}

/// Neutral frames at the tail of the delay: a still-held guard bleeds into
/// the macro's chord (MK's held Block eats attack buttons — live-observed:
/// the slide fires from a clean simultaneous press, not from Block-held +
/// buttons added), so everything is released before the first step.
const PUNISH_RELEASE: u64 = 4;

/// Frames an in-flight punish (delay + macro) may keep running while the
/// gate is closed (MK2 zeroes its in-fight word from the contact frame
/// onward) before it is dropped as a real round end.
/// Quiet frames required to re-arm the trigger after a punish. The contact
/// signal (a health delta) moves for a SINGLE frame per hit, so this only has
/// to outlast the write itself; the old value reused HITSTUN_RECENT_FRAMES
/// (20 — the hitstun FEATURE window, a different concept) and made
/// back-to-back pressure feel unresponsive.
const PUNISH_REARM_FRAMES: u64 = 8;

const PUNISH_GATE_GRACE: u64 = 60;

/// One BlockPunish frame: guard by policy; on each TRIGGER sample the weighted
/// pool and play the pick through [`crate::macros::MacroExec`] on the dummy
/// port. The trigger is family-dependent:
/// - button families: the contact signal changing while guarding (§6) — the
///   dummy knows it was actually struck;
/// - back-hold families: the guard window OPENING (§9.3) — blocked contact is
///   undetectable there (zero chip, quiet counters), so the honest trigger is
///   "the opponent committed an attack inside `guard_range`". That is a
///   SUPERSET drill (block-punish AND whiff-punish) and the phase string says
///   "punishing (commit)" so nothing implies confirmed contact.
///
/// The cooldown re-arms only after the trigger source has been quiet
/// ≥ [`PUNISH_REARM_FRAMES`], so one attack (or one blocked string) triggers
/// one punish, not one per frame.
fn block_punish(
    ds: &mut DebugState,
    frame: u64,
    p: &GameProfile,
    r: &Resolved,
    g: GuardOut,
    contact: (bool, u8),
) -> [bool; 12] {
    use crate::macros::PunishOption;
    let guard_bits = g.bits;
    let dummy_block = DUMMY_BLOCK;
    // §9.3: back-hold families trigger on commitment, button families on contact.
    let on_commit = matches!(r.guard, Guard::BackHold { .. });
    let (changed, cur) = contact;
    let trigger = if on_commit { g.commit_edge } else { changed };
    if on_commit && g.commit {
        // The open window is this path's "signal is live"; the cooldown counts
        // quiet frames from the moment it closes.
        ds.training.punish_last_change = frame;
    }
    if !on_commit && r.contact.is_none() {
        return guard_bits; // no trigger mapped — degrade to plain Block
    }
    if !ds.training.punish_armed
        && frame.saturating_sub(ds.training.punish_last_change) >= PUNISH_REARM_FRAMES
    {
        ds.training.punish_armed = true;
    }
    // Re-derived every frame — a side switch mid-macro flips "back" with it.
    let opp_right = g.opp_right;

    // An in-flight punish: guard out the post-contact delay, then play the
    // macro to completion.
    let mut out: Option<[bool; 12]> = None;
    if ds.training.punish_exec.is_some() {
        if ds.training.punish_wait > 0 {
            ds.training.punish_wait -= 1;
            // Guard through blockstun (out = None falls through to the guard
            // hold), then release everything for a clean chord press.
            if ds.training.punish_wait < PUNISH_RELEASE {
                out = Some([false; 12]);
            }
        } else {
            let mut done = false;
            if let Some(ex) = ds.training.punish_exec.as_mut() {
                match ex.next(opp_right) {
                    Some(bits) => out = Some(bits),
                    None => done = true,
                }
            }
            if done {
                ds.training.punish_exec = None;
            }
        }
    }

    if ds.training.punish_exec.is_none() {
        let mode = ds.training.guard_mode;
        ds.training.punish_phase = if frame < ds.training.punish_hold_until {
            // ContinueBlock: keep guarding (reactively, where that is the
            // policy) and decline to punish until the hold expires.
            if on_commit {
                "guarding — continue block".to_string()
            } else {
                "guarding — holding block".to_string()
            }
        } else if !ds.training.punish_armed {
            let quiet = frame.saturating_sub(ds.training.punish_last_change);
            format!("cooling — {}f", PUNISH_REARM_FRAMES.saturating_sub(quiet))
        } else if g.guarding {
            // "(window)" is only meaningful where the guard is reactive.
            if on_commit { "guarding (window) — ARMED".into() } else { "guarding — ARMED".into() }
        } else if g.commit {
            // An opportunity the MODE declined — say so, don't look broken.
            format!("not guarding ({}) — ARMED", mode.label())
        } else {
            "neutral — ARMED".to_string()
        };
    }

    if ds.training.punish_exec.is_none()
        && trigger
        && ds.training.punish_armed
        && frame >= ds.training.punish_hold_until
    {
        ds.training.punish_armed = false;
        // A commit trigger never confirmed contact — the phase must not
        // pretend it did (§9.3).
        let verb = if on_commit { "punishing (commit)" } else { "punishing" };
        // Never deterministic (§6): wall-clock entropy in the seed.
        let seed = frame ^ ((cur as u64) << 32) ^ entropy();
        let pick = crate::macros::weighted_pick(&ds.training.punish_pool, seed).cloned();
        // Scheduled, not stepped: the macro's first input lands `delay`
        // frames from now (ReversalTiming-resolved: Fast/Late/Explicit are
        // fixed, Delay re-rolls right here on a seed distinct from the pool
        // pick above), after hit-freeze + blockstun have passed.
        let delay = resolve_reversal_delay(ds.training.reversal_timing, seed ^ 0xD1B5_4A32_D192_ED03);
        let start = move |m: crate::macros::CompiledMacro, ds: &mut DebugState| {
            ds.training.punish_exec = Some(crate::macros::MacroExec::new(m));
            ds.training.punish_wait = delay;
        };
        match pick {
            Some(PunishOption::Move(name)) => {
                let char_id = p
                    .field_addr(dummy_block, "char_id")
                    .map(|(addr, _)| p.canon_char_id(rd8(ds, addr)));
                let steps = char_id.and_then(|id| {
                    p.specials_for(id)
                        .into_iter()
                        .find(|(n, _)| *n == name)
                        .map(|(_, s)| s.to_vec())
                });
                match steps.and_then(|s| crate::macros::compile(&name, &s, p).ok()) {
                    Some(m) => {
                        start(m, ds);
                        ds.training.punish_phase = format!("{verb}: {name}");
                        ds.log(format!("🎯 punish: {name}"));
                        eprintln!("[training] punish: {name}"); // headless-visible twin
                    }
                    // Stale pool (character changed since the panel built it).
                    None => ds.log(format!("🎯 punish: '{name}' not encoded for this character")),
                }
            }
            Some(PunishOption::Attack(class)) => {
                let spec = crate::profile::StepSpec {
                    dirs: Vec::new(),
                    press: vec![class.clone()],
                    hold: Vec::new(),
                    release: Vec::new(),
                    while_held: Vec::new(),
                    frames: 3,
                    min_frames: None,
                };
                if let Ok(m) = crate::macros::compile(&class, &[spec], p) {
                    start(m, ds);
                    ds.training.punish_phase = format!("{verb}: {class}");
                    ds.log(format!("🎯 punish: {class}"));
                    eprintln!("[training] punish: {class}"); // headless-visible twin
                }
            }
            Some(PunishOption::ContinueBlock(n)) => {
                ds.training.punish_hold_until = frame + n as u64;
                ds.log("🎯 punish: continue block".into());
            }
            None => {} // empty/zero-weight pool: just keep guarding
        }
    }
    out.unwrap_or(guard_bits)
}

/// The dummy's CANONICAL char id (its block's `char_id` through `id_map`) —
/// what the panel keys `specials_for` on. None while training is unavailable
/// or the port maps no `char_id`.
pub fn punish_dummy_char(ds: &DebugState) -> Option<u8> {
    let p = crate::profile::current();
    resolve(p)?;
    let (addr, _) = p.field_addr(DUMMY_BLOCK, "char_id")?;
    Some(p.canon_char_id(rd8(ds, addr)))
}

/// Whether the loaded family guards REACTIVELY (back-hold) — the panel words
/// its hints differently for the two styles.
pub fn guard_is_reactive() -> bool {
    resolve(crate::profile::current())
        .map(|r| matches!(r.guard, Guard::BackHold { .. }))
        .unwrap_or(false)
}

/// The port's guard range in `x` units, when the reactive guard uses one.
pub fn guard_range() -> Option<i32> {
    match resolve(crate::profile::current())?.guard {
        Guard::BackHold { range, .. } => range,
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn tick_is_inert_when_disabled_or_out_of_fight() {
        crate::profile::init_for_tests();
        let mut ds = DebugState::new();
        // disabled: nothing queued
        tick(&mut ds, 0);
        assert!(ds.pending_bus_writes.is_empty());
        // enabled but bare state (all reads 0 → not in fight): only the
        // credits top-up may fire on frame 0 — and it writes nothing because
        // there is no writable region, so the queue stays empty.
        ds.training.enabled = true;
        tick(&mut ds, 0);
        assert!(ds.pending_bus_writes.is_empty());
        assert_eq!(ds.injected_input2, [0u16; 12]);
    }

    #[test]
    fn asurabld_supports_every_feature() {
        let p = crate::profile::init_for_tests();
        let f = features_of(p).expect("asurabld must be training-available");
        assert!(f.refill && f.timer_hold && f.credits);
        assert!(f.position_reset && f.finish_round && f.block_dummy);
        assert!(f.block_punish, "hitstun_sources + x pair → BlockPunish available");
        assert!(f.missing().is_empty());
    }

    #[test]
    fn mk2_degrades_per_feature() {
        // MK2's map is partial by honesty (mk2.md): gate + health + world X/Y
        // exist (X/Y now pointer-resolved, docs/frames.md §5); timer store,
        // credits (CMOS), and round_state don't. `positions` (round-start
        // teleport coordinates) is also unmapped, so position_reset stays
        // declined regardless of x/y being resolvable.
        let p = GameProfile::load(Path::new("library/mk2")).expect("mk2 profile loads");
        let f = features_of(&p).expect("mk2 must be training-available (has a gate)");
        assert!(f.refill, "health field + HUD pair are mapped");
        assert!(f.block_dummy, "MK is button-block: the Block chord is mapped");
        assert!(f.block_punish, "hitstun_sources (HUD health pair) is mapped");
        assert!(!f.timer_hold && !f.credits && !f.position_reset && !f.finish_round);
        assert_eq!(
            f.missing(),
            vec!["timer hold", "credits top-up", "position reset", "finish round"]
        );
        // And the refill spec includes all four MK2 health bytes.
        let r = resolve(&p).unwrap();
        let rf = r.refill.unwrap();
        let all: Vec<u32> = rf.sides.iter().flat_map(|s| s.addrs.iter().copied()).collect();
        assert_eq!(all.len(), 4, "struct pair + HUD pair: {all:x?}");
        // MK is button-block: the dummy must hold the Block chord (L), not
        // walk backward.
        let Guard::Button(chord) = r.guard else {
            panic!("mk2 must resolve a button guard policy");
        };
        let held: Vec<usize> = chord.iter().enumerate().filter(|(_, on)| **on).map(|(i, _)| i).collect();
        assert_eq!(held, vec![10], "Block = RETRO L");
    }

    #[test]
    fn genesis_pins_resolve_to_pad_mode_flags() {
        let p = GameProfile::load(Path::new("library/mk2/genesis")).expect("genesis loads");
        let pins = p.resolved_pins();
        assert_eq!(pins, vec![(0xFFF9D1, 1), (0xFFF9D0, 1)],
                   "both 6-button flags pinned on (mk2-genesis.md)");
        // asurabld declares no pins.
        assert!(crate::profile::init_for_tests().resolved_pins().is_empty());
    }

    /// Stage a synthetic MK2-shaped object pool entry for `block`'s pointer
    /// (docs/frames.md §5): writes the raw pointer word at `base - 0xC`, the
    /// block's own `char_id`, the cross-check byte at `obj + 0x3E` (made to
    /// match), and `x` at `obj + 0x12` — everything `read_object_ptr` needs
    /// to resolve live through the real decode, not a hardcoded fallback.
    fn stage_object_ptr(ds: &mut DebugState, base: u32, char_id: u8, obj: u32, x: u16) {
        let raw_ptr = 0x0100_0000u32 + (obj << 3);
        assert!(ds.write_addr(base.wrapping_sub(0xC) as usize, 4, raw_ptr));
        assert!(ds.write_addr(base as usize, 1, char_id as u32));
        assert!(ds.write_addr((obj + 0x3E) as usize, 1, char_id as u32));
        assert!(ds.write_addr((obj + 0x12) as usize, 2, x as u32));
    }

    /// The full §6 loop against the mk2 profile: arm on quiet, trigger on a
    /// hit_counter change while guarding, play the char-aware slide through
    /// MacroExec on the dummy port, return to the guard chord, and stay in
    /// cooldown until the signal goes quiet again.
    #[test]
    fn block_punish_fires_the_slide_on_contact_then_cools_down() {
        let p = GameProfile::load(Path::new("library/mk2")).expect("mk2 profile loads");
        let mut ds = DebugState::new();
        assert!(ds.install_bus_window(crate::debug::BusWindowCfg {
            name: "wram-punish-test".into(),
            addr: 0x0,
            len: 0x10000,
            interval: 1,
            flags: crate::libretro::RETRO_MEMDESC_SYSTEM_RAM,
        }));
        // Open mk2's gate and stage the matchup: dummy (higher X → block2)
        // is Reptile; its contact signal is the block2 hitstun source
        // (p2_health_hud — the HUD accumulator that moves when P2 is struck).
        let hoff = p.field_off("health").unwrap().0;
        assert!(ds.write_addr((p.block1() + hoff) as usize, 1, 100));
        assert!(ds.write_addr((p.block2() + hoff) as usize, 1, 90));
        // x/y are pointer-resolved (docs/frames.md §5) — stage a real object
        // pool entry per block so `opp_right` (used for the slide's "back"
        // direction) resolves through the actual decode, not a fallback:
        // block1 at x=100 (left), block2/dummy (Reptile) at x=200 (right).
        stage_object_ptr(&mut ds, p.block1(), 0, 0x7000, 100);
        stage_object_ptr(&mut ds, p.block2(), 9, 0x7100, 200);
        // The dummy is block2 (larger x); mk2 ships no contact_signal, so
        // the trigger falls back to hitstun_sources — block2's HUD health.
        // (A blocking MK2 fighter's struct is otherwise frozen and blocked
        // contact always chips, so the health delta IS the contact event —
        // see mk2.md's contact-signal investigation.)
        let sig = p.global("p2_health_hud").unwrap() as usize;
        assert!(crate::gate::eval_gate(&ds, &p));

        ds.training.enabled = true;
        ds.training.dummy = crate::debug::DummyMode::BlockPunish;
        ds.training.punish_pool =
            vec![(crate::macros::PunishOption::Move("slide".into()), 1)];

        // Quiet frames arm the trigger; meanwhile the dummy holds the guard
        // chord (Block = RETRO L, bit 10).
        for f in 1..=25 {
            tick_with(&mut ds, f, &p);
        }
        assert!(ds.training.punish_armed);
        assert_eq!(ds.injected_input2[10], 2, "guarding while armed");
        assert_eq!(ds.injected_input2[8], 0);

        // Contact: the signal moves while the dummy guards → punish is
        // SCHEDULED (guard held through hit-freeze + blockstun first).
        assert!(ds.write_addr(sig, 1, 1));
        tick_with(&mut ds, 26, &p);
        assert!(ds.training.punish_exec.is_some(), "punish scheduled");
        assert_eq!(ds.training.punish_wait, PUNISH_DELAY);
        assert_eq!(ds.injected_input2[10], 2, "still guarding through the delay");
        assert!(!ds.training.punish_armed, "trigger disarms");

        // Transient gate closure — the scheduled punish must ride it out
        // under grace instead of stalling. 262 = char select/ladder (the
        // documented arcade menu value: bit 0x02 set); the 2-human in-fight
        // values 260/276 have it CLEAR and are legal — see mk2.md's gate
        // revisions.
        let scr = p.global("screen_state").unwrap() as usize;
        assert!(ds.write_addr(scr, 2, 262));
        assert!(!crate::gate::eval_gate(&ds, &p));
        let mut f = 27;
        tick_with(&mut ds, f, &p);
        assert_eq!(ds.injected_input2[10], 2, "guard held while riding out blockstun");
        f += 1;
        while ds.training.punish_wait > 0 {
            tick_with(&mut ds, f, &p);
            f += 1;
        }
        // The release tail dropped everything for a clean chord press.
        assert_eq!(ds.injected_input2[10], 0, "guard released before the chord");
        // Delay drained: the slide plays under the closed gate. Frame 1:
        // back (dummy is the RIGHT fighter, opponent left → back = Right,
        // bit 7) + LK (a, 8) + LP (b, 0) + Block (l, 10 — part of the chord).
        tick_with(&mut ds, f, &p);
        assert_eq!(ds.injected_input2[7], 2);
        assert_eq!(ds.injected_input2[8], 2);
        assert_eq!(ds.injected_input2[0], 2);
        assert_eq!(ds.injected_input2[10], 2, "Block is part of the verified chord");
        for i in 1..=8 {
            tick_with(&mut ds, f + i, &p);
        }
        assert!(ds.training.punish_exec.is_none(), "macro finished under grace");
        f += 9;

        // Gate reopens: the guard chord returns.
        assert!(ds.write_addr(scr, 2, 0));
        tick_with(&mut ds, f, &p);
        assert_eq!(ds.injected_input2[10], 2, "back to guarding");
        assert_eq!(ds.injected_input2[8], 0);

        // Another change inside the quiet window must NOT re-trigger.
        assert!(ds.write_addr(sig, 1, 2));
        tick_with(&mut ds, f + 1, &p);
        assert_eq!(ds.injected_input2[10], 2, "cooldown holds the guard");
        assert!(!ds.training.punish_armed);
    }

    #[test]
    fn asurabld_blocks_by_holding_back_not_a_chord() {
        let p = crate::profile::init_for_tests();
        let r = resolve(p).unwrap();
        assert!(r.x_pair.is_some());
        // back_hold → the REACTIVE policy, keyed on the `attacking` field
        // (the port maps it) and gated by the live-measured guard range.
        match r.guard {
            Guard::BackHold { range, commit, tail } => {
                assert_eq!(range, Some(175), "asurabld.md guard_range");
                assert!(matches!(commit, Commit::Field(..)), "attacking (+0x6F) is mapped");
                assert_eq!(tail, GUARD_FIELD_TAIL);
            }
            _ => panic!("asurabld must resolve a reactive back-hold guard"),
        }
    }

    // ── the reactive guard (MACRO_ACTIONS §9) ───────────────────────────────

    /// asurabld staged in a bus window with an OPEN gate: the scenario every
    /// guard test drives by moving x / `attacking` around.
    fn asurabld_scene() -> (GameProfile, DebugState) {
        let p = GameProfile::load(Path::new("library/asurabld")).expect("asurabld loads");
        let mut ds = DebugState::new();
        assert!(ds.install_bus_window(crate::debug::BusWindowCfg {
            name: "wram-guard-test".into(),
            addr: 0x400000,
            len: 0x10000,
            interval: 1,
            flags: crate::libretro::RETRO_MEMDESC_SYSTEM_RAM,
        }));
        let hoff = p.field_off("health").unwrap().0;
        assert!(ds.write_addr((p.block1() + hoff) as usize, 1, 100));
        assert!(ds.write_addr((p.block2() + hoff) as usize, 1, 100));
        assert!(ds.write_addr(p.global("round_timer").unwrap() as usize, 1, 0x99));
        assert!(crate::gate::eval_gate(&ds, &p), "gate must be open for the guard tests");
        ds.training.enabled = true;
        ds.training.refill = false;
        (p, ds)
    }

    /// Place the fighters (big-endian words) and set the OPPONENT's (block 1's)
    /// attack-commitment flag.
    fn stage(ds: &mut DebugState, p: &GameProfile, opp_x: u16, me_x: u16, attacking: u8) {
        let xoff = p.field_off("x").unwrap().0;
        let aoff = p.field_off("attacking").unwrap().0;
        assert!(ds.write_addr((p.block1() + xoff) as usize, 2, opp_x.swap_bytes() as u32));
        assert!(ds.write_addr((p.block2() + xoff) as usize, 2, me_x.swap_bytes() as u32));
        assert!(ds.write_addr((p.block1() + aoff) as usize, 1, attacking as u32));
    }

    fn held(ds: &DebugState) -> Vec<usize> {
        ds.injected_input2.iter().enumerate().filter(|(_, v)| **v > 0).map(|(i, _)| i).collect()
    }

    /// THE acceptance property (§9.1): a back-hold dummy that is not being
    /// attacked asserts NOTHING, so it cannot walk itself into the corner —
    /// the bug that motivated this whole section (165 → 286 units in ~1 s).
    #[test]
    fn back_hold_guard_asserts_nothing_while_the_opponent_idles() {
        let (p, mut ds) = asurabld_scene();
        ds.training.dummy = DummyMode::Block;
        stage(&mut ds, &p, 200, 300, 0);
        for f in 1..=300 {
            tick_with(&mut ds, f, &p);
            assert!(held(&ds).is_empty(), "frame {f}: reactive guard must stand neutral");
        }
    }

    /// The guard window: opponent attacking AND inside `guard_range` → hold
    /// away, PURE standing back (never Down — down-back blocks nothing here,
    /// 0/4 reproduced). Out of range → nothing. Released when the attack ends.
    #[test]
    fn back_hold_guard_opens_only_on_a_committed_attack_in_range() {
        let (p, mut ds) = asurabld_scene();
        ds.training.dummy = DummyMode::Block;
        let mut f = 0u64;
        let mut step = |ds: &mut DebugState, f: &mut u64| {
            *f += 1;
            tick_with(ds, *f, &p);
        };

        // In range (gap 100 ≤ 175), attacking → away = Right (dummy on the right).
        stage(&mut ds, &p, 200, 300, 1);
        step(&mut ds, &mut f);
        assert_eq!(held(&ds), vec![BIT_RIGHT], "guarding: pure standing back");

        // Attack ends: the window closes after the (latency-only) tail.
        stage(&mut ds, &p, 200, 300, 0);
        for _ in 0..=GUARD_FIELD_TAIL {
            step(&mut ds, &mut f);
        }
        assert!(held(&ds).is_empty(), "released once the opponent recovers");

        // Same attack, but OUT of range (gap 250 > 175): far whiffs are ignored.
        stage(&mut ds, &p, 200, 450, 1);
        for _ in 0..5 {
            step(&mut ds, &mut f);
        }
        assert!(held(&ds).is_empty(), "out of guard_range → no window, no drift");

        // Sides swapped (dummy on the LEFT): away = Left, still never Down.
        stage(&mut ds, &p, 300, 200, 1);
        step(&mut ds, &mut f);
        assert_eq!(held(&ds), vec![BIT_LEFT], "away follows live sides");
    }

    /// Guard modes (§9.4). None never guards; After First Hit guards only once
    /// a contact signal has fired, for the rest of the string; Random is
    /// weighted (statistically, and exactly at the 0 %/100 % edges).
    #[test]
    fn guard_modes_gate_the_window() {
        let (p, mut ds) = asurabld_scene();
        ds.training.dummy = DummyMode::Block;
        stage(&mut ds, &p, 200, 300, 1); // a permanent guard opportunity

        // None: never.
        ds.training.guard_mode = GuardMode::None;
        for f in 1..=30 {
            tick_with(&mut ds, f, &p);
            assert!(held(&ds).is_empty(), "GuardMode::None must never guard");
        }

        // After First Hit: nothing until the dummy's contact signal moves.
        ds.training.guard_mode = GuardMode::AfterFirstHit;
        for f in 31..=60 {
            tick_with(&mut ds, f, &p);
            assert!(held(&ds).is_empty(), "no hit yet → no guard");
        }
        let sig = p.global("combo_on_b2").unwrap() as usize; // block 2 = the dummy
        assert!(ds.write_addr(sig, 1, 1));
        tick_with(&mut ds, 61, &p);
        assert_eq!(held(&ds), vec![BIT_RIGHT], "first hit landed → guard the rest of the string");
        for f in 62..=70 {
            tick_with(&mut ds, f, &p);
            assert_eq!(held(&ds), vec![BIT_RIGHT], "still inside the string");
        }
        // The string ends once the contact signal has been quiet longer than
        // the hitstun window — the dummy eats the next first hit again.
        for f in 71..=140 {
            tick_with(&mut ds, f, &p);
        }
        assert!(held(&ds).is_empty(), "string over → back to eating the first hit");

        // Random at the deterministic edges.
        ds.training.guard_mode = GuardMode::Random;
        ds.training.guard_random_pct = crate::debug::GuardPct(0);
        for f in 141..=170 {
            tick_with(&mut ds, f, &p);
            assert!(held(&ds).is_empty(), "0 % never guards");
        }
        ds.training.guard_random_pct = crate::debug::GuardPct(100);
        ds.training.guard_roll = None;
        tick_with(&mut ds, 171, &p);
        assert_eq!(held(&ds), vec![BIT_RIGHT], "100 % always guards");
    }

    /// Random is a WEIGHTED coin per opportunity (not per frame): each attack
    /// is guarded or not, wholesale. Statistical assertion — loose bounds.
    #[test]
    fn guard_mode_random_is_weighted_per_opportunity() {
        let (p, mut ds) = asurabld_scene();
        ds.training.dummy = DummyMode::Block;
        ds.training.guard_mode = GuardMode::Random;
        ds.training.guard_random_pct = crate::debug::GuardPct(50);
        let mut f = 0u64;
        let (mut guarded, mut trials) = (0u32, 0u32);
        for _ in 0..300 {
            // One opportunity: the attack starts, is held, then recovers.
            stage(&mut ds, &p, 200, 300, 1);
            let mut taken = None;
            for _ in 0..4 {
                f += 1;
                tick_with(&mut ds, f, &p);
                let now = !held(&ds).is_empty();
                // Sticky: the decision must not flicker inside one attack.
                match taken {
                    None => taken = Some(now),
                    Some(prev) => assert_eq!(prev, now, "the roll must be sticky per attack"),
                }
            }
            trials += 1;
            if taken == Some(true) {
                guarded += 1;
            }
            stage(&mut ds, &p, 200, 300, 0);
            for _ in 0..=GUARD_FIELD_TAIL {
                f += 1;
                tick_with(&mut ds, f, &p);
            }
        }
        assert!(
            guarded > trials / 5 && guarded < trials * 4 / 5,
            "≈50 % of {trials} opportunities should be guarded, got {guarded}"
        );
    }

    /// §9.3: back-hold families punish on the guard window OPENING (blocked
    /// contact is undetectable), and the phase string must say so.
    #[test]
    fn back_hold_punishes_on_attack_commitment_not_contact() {
        let (p, mut ds) = asurabld_scene();
        ds.training.dummy = DummyMode::BlockPunish;
        ds.training.punish_pool = vec![(crate::macros::PunishOption::Attack("Medium".into()), 1)];
        stage(&mut ds, &p, 200, 300, 0);

        // Quiet frames arm the trigger while the dummy stands NEUTRAL.
        for f in 1..=20 {
            tick_with(&mut ds, f, &p);
        }
        assert!(ds.training.punish_armed);
        assert!(held(&ds).is_empty(), "armed but not guarding: nothing is asserted");
        assert_eq!(ds.training.punish_phase, "neutral — ARMED");

        // The opponent commits an attack in range: guard + schedule the punish.
        stage(&mut ds, &p, 200, 300, 1);
        tick_with(&mut ds, 21, &p);
        assert!(ds.training.punish_exec.is_some(), "commit edge schedules the punish");
        assert_eq!(ds.training.punish_wait, PUNISH_DELAY);
        assert!(
            ds.training.punish_phase.starts_with("punishing (commit)"),
            "must not imply confirmed contact: {}",
            ds.training.punish_phase
        );
        assert_eq!(held(&ds), vec![BIT_RIGHT], "keeps guarding through the delay");

        // Ride out the delay, then the macro presses Medium (RETRO a = bit 8).
        let mut f = 22;
        while ds.training.punish_wait > 0 {
            tick_with(&mut ds, f, &p);
            f += 1;
        }
        assert!(held(&ds).is_empty(), "released for a clean press");
        tick_with(&mut ds, f, &p);
        assert_eq!(held(&ds), vec![8], "Medium = RETRO a");

        // One attack, one punish: the window stays open but never re-triggers.
        while ds.training.punish_exec.is_some() {
            f += 1;
            tick_with(&mut ds, f, &p);
        }
        f += 1;
        tick_with(&mut ds, f, &p);
        assert!(!ds.training.punish_armed, "still cooling while the attack is live");
        assert!(ds.training.punish_exec.is_none());
    }

    /// The button path's phase wording stays contact-flavoured (no "(commit)")
    /// — the two families must never borrow each other's honesty claims.
    #[test]
    fn button_punish_phase_says_contact_not_commit() {
        let p = GameProfile::load(Path::new("library/mk2")).expect("mk2 profile loads");
        let r = resolve(&p).unwrap();
        assert!(matches!(r.guard, Guard::Button(_)));
        let mut ds = DebugState::new();
        assert!(ds.install_bus_window(crate::debug::BusWindowCfg {
            name: "wram-phase-test".into(),
            addr: 0x0,
            len: 0x10000,
            interval: 1,
            flags: crate::libretro::RETRO_MEMDESC_SYSTEM_RAM,
        }));
        let hoff = p.field_off("health").unwrap().0;
        assert!(ds.write_addr((p.block1() + hoff) as usize, 1, 100));
        assert!(ds.write_addr((p.block2() + hoff) as usize, 1, 90));
        assert!(ds.write_addr(p.global("p1_x").unwrap() as usize, 2, 100));
        assert!(ds.write_addr(p.global("p2_x").unwrap() as usize, 2, 200));
        ds.training.enabled = true;
        ds.training.dummy = DummyMode::BlockPunish;
        ds.training.punish_pool = vec![(crate::macros::PunishOption::Attack("HP".into()), 1)];
        for f in 1..=20 {
            tick_with(&mut ds, f, &p);
        }
        assert_eq!(ds.training.punish_phase, "guarding — ARMED");
        let sig = p.global("p2_health_hud").unwrap() as usize;
        assert!(ds.write_addr(sig, 1, 1));
        tick_with(&mut ds, 21, &p);
        assert_eq!(ds.training.punish_phase, "punishing: HP");
    }

    // ── ReversalTiming (Fast / Delay / Late / Explicit) ─────────────────────

    /// Pure resolution logic, no scene needed: every mode maps to the
    /// expected frame count, and `Delay` clamps into `[min, max]` regardless
    /// of argument order (a reversed pair is swapped, not rejected).
    #[test]
    fn resolve_reversal_delay_covers_every_mode() {
        use crate::debug::ReversalTiming::*;
        assert_eq!(resolve_reversal_delay(Fast, 0), PUNISH_DELAY_FAST);
        assert_eq!(resolve_reversal_delay(Late, 12345), PUNISH_DELAY_LATE);
        assert_eq!(resolve_reversal_delay(Explicit(99), 0), 99);
        for seed in 0..50u64 {
            let v = resolve_reversal_delay(Delay { min: 30, max: 10 }, seed);
            assert!((10..=30).contains(&v), "seed {seed} -> {v} out of [10,30]");
        }
        assert_eq!(resolve_reversal_delay(Delay { min: 5, max: 5 }, 999), 5);
    }

    /// `Fast` resolves to the measured floor AND the floor still actually
    /// produces a live punish press — i.e. the floor is not below what the
    /// game accepts (a lower value would be eaten by hit-freeze/blockstun,
    /// per `PUNISH_DELAY_FAST`'s doc comment).
    #[test]
    fn reversal_timing_fast_uses_the_floor_and_still_punishes() {
        let (p, mut ds) = asurabld_scene();
        ds.training.dummy = DummyMode::BlockPunish;
        ds.training.reversal_timing = crate::debug::ReversalTiming::Fast;
        ds.training.punish_pool = vec![(crate::macros::PunishOption::Attack("Medium".into()), 1)];
        stage(&mut ds, &p, 200, 300, 0);
        for f in 1..=20 {
            tick_with(&mut ds, f, &p);
        }
        assert!(ds.training.punish_armed);

        stage(&mut ds, &p, 200, 300, 1); // commit edge triggers the punish
        tick_with(&mut ds, 21, &p);
        assert!(ds.training.punish_exec.is_some(), "commit edge schedules the punish");
        assert_eq!(ds.training.punish_wait, PUNISH_DELAY_FAST, "Fast resolves to the measured floor");

        let mut f = 22;
        while ds.training.punish_wait > 0 {
            tick_with(&mut ds, f, &p);
            f += 1;
        }
        assert!(held(&ds).is_empty(), "released for a clean press");
        tick_with(&mut ds, f, &p);
        assert_eq!(held(&ds), vec![8], "Fast's floor still produces a live punish press (Medium = RETRO a)");
    }

    /// `Late` resolves to its configured (global-calibration) ceiling value.
    #[test]
    fn reversal_timing_late_resolves_to_the_configured_ceiling() {
        let (p, mut ds) = asurabld_scene();
        ds.training.dummy = DummyMode::BlockPunish;
        ds.training.reversal_timing = crate::debug::ReversalTiming::Late;
        ds.training.punish_pool = vec![(crate::macros::PunishOption::Attack("Medium".into()), 1)];
        stage(&mut ds, &p, 200, 300, 0);
        for f in 1..=20 {
            tick_with(&mut ds, f, &p);
        }
        assert!(ds.training.punish_armed);

        stage(&mut ds, &p, 200, 300, 1);
        tick_with(&mut ds, 21, &p);
        assert!(ds.training.punish_exec.is_some());
        assert_eq!(ds.training.punish_wait, PUNISH_DELAY_LATE);
    }

    /// `Delay` re-rolls a fresh value in `[min, max]` on every scheduled
    /// punish, end to end through `block_punish` (live entropy, not the pure
    /// `resolve_reversal_delay` unit test above).
    #[test]
    fn reversal_timing_delay_stays_within_its_range_across_repeated_punishes() {
        let (p, mut ds) = asurabld_scene();
        ds.training.dummy = DummyMode::BlockPunish;
        let (min, max) = (10u64, 20u64);
        ds.training.reversal_timing = crate::debug::ReversalTiming::Delay { min, max };
        ds.training.punish_pool = vec![(crate::macros::PunishOption::Attack("Medium".into()), 1)];

        let mut f = 0u64;
        let mut waits = Vec::new();
        for _ in 0..12 {
            // Quiet long enough to (re)arm the trigger — the previous trial's
            // commit window has a `GUARD_FIELD_TAIL`-frame hangover after
            // `attacking` drops back to 0, so this needs real margin beyond
            // `PUNISH_REARM_FRAMES` (matches the generous 20f used by the
            // single-shot fixtures above).
            stage(&mut ds, &p, 200, 300, 0);
            for _ in 0..20 {
                f += 1;
                tick_with(&mut ds, f, &p);
            }
            assert!(ds.training.punish_armed);
            // Commit edge triggers the punish.
            stage(&mut ds, &p, 200, 300, 1);
            f += 1;
            tick_with(&mut ds, f, &p);
            assert!(ds.training.punish_exec.is_some());
            waits.push(ds.training.punish_wait);
            // Drain to completion before the next trial.
            while ds.training.punish_exec.is_some() {
                f += 1;
                tick_with(&mut ds, f, &p);
            }
        }

        assert!(waits.iter().all(|&w| (min..=max).contains(&w)), "{waits:?} out of [{min},{max}]");
        assert!(
            waits.iter().any(|&w| w != waits[0]),
            "Delay must re-roll per punish, not stick to one value: {waits:?}"
        );
    }

    // ── input-slot playback precedence (task A2 §4) ─────────────────────────

    /// When an input-slot playback is actively driving P2, the training
    /// dummy's write to `injected_input2` must be suppressed entirely — not
    /// blended with playback's `held_input2` via the fold's OR. This is the
    /// explicit "one must win, visibly" precedence task A2 requires.
    #[test]
    fn playback_wins_over_the_dummy_on_p2() {
        let p = GameProfile::load(Path::new("library/mk2")).expect("mk2 profile loads");
        let mut ds = DebugState::new();
        assert!(ds.install_bus_window(crate::debug::BusWindowCfg {
            name: "wram-playback-precedence-test".into(),
            addr: 0x0,
            len: 0x10000,
            interval: 1,
            flags: crate::libretro::RETRO_MEMDESC_SYSTEM_RAM,
        }));
        let hoff = p.field_off("health").unwrap().0;
        assert!(ds.write_addr((p.block1() + hoff) as usize, 1, 100));
        assert!(ds.write_addr((p.block2() + hoff) as usize, 1, 90));
        assert!(ds.write_addr(p.global("p1_x").unwrap() as usize, 2, 100));
        assert!(ds.write_addr(p.global("p2_x").unwrap() as usize, 2, 200));
        assert!(crate::gate::eval_gate(&ds, &p), "gate must be open for this test");

        ds.training.enabled = true;
        // MK2 is button-block: with no playback, the dummy holds Block (L,
        // bit 10) unconditionally — see `mk2_degrades_per_feature`.
        ds.training.dummy = DummyMode::Block;

        // Arm a MANUAL playback targeting P2 that presses Up (bit 4) — a
        // button the dummy would never press on its own, so the two writes
        // are trivially distinguishable.
        let slot = crate::playback::InputSlot {
            version: crate::playback::SLOT_VERSION,
            family: p.family.family.clone(),
            port: p.port.port.clone(),
            created_at: 1,
            state_note_at_start: None,
            frames: vec![[0, 1 << 4], [0, 1 << 4]],
        };
        let path = crate::playback::save_slot(&slot, "precedence-test").unwrap();
        crate::playback::start_playback(
            &mut ds,
            "precedence-test",
            crate::debug::PlaybackPort::P2,
            crate::debug::PlaybackTrigger::Manual,
            &p,
        )
        .expect("start_playback");

        tick_with(&mut ds, 1, &p);
        assert!(ds.held_input2[4], "playback's Up came through via held_input2");
        assert_eq!(
            ds.injected_input2[10], 0,
            "the dummy's Block write must be SUPPRESSED, not blended in alongside playback"
        );

        let _ = std::fs::remove_file(path);
    }

    /// With NO playback active, the precedence check is a no-op: the dummy
    /// behaves exactly as before this feature existed.
    #[test]
    fn dummy_is_unaffected_when_no_playback_is_active() {
        let p = GameProfile::load(Path::new("library/mk2")).expect("mk2 profile loads");
        let mut ds = DebugState::new();
        assert!(ds.install_bus_window(crate::debug::BusWindowCfg {
            name: "wram-no-playback-test".into(),
            addr: 0x0,
            len: 0x10000,
            interval: 1,
            flags: crate::libretro::RETRO_MEMDESC_SYSTEM_RAM,
        }));
        let hoff = p.field_off("health").unwrap().0;
        assert!(ds.write_addr((p.block1() + hoff) as usize, 1, 100));
        assert!(ds.write_addr((p.block2() + hoff) as usize, 1, 90));
        ds.training.enabled = true;
        ds.training.dummy = DummyMode::Block;
        tick_with(&mut ds, 1, &p);
        assert_eq!(ds.injected_input2[10], 2, "Block chord asserted normally with no playback");
    }
}
