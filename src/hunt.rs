//! Signal hunt — event-marked differential RAM analysis.
//!
//! `docs/signal-hunt.md` is the NORMATIVE contract; this module implements it.
//! It automates the manual protocol that was hand-run five times in one session
//! (MK2 hit counter, MK2 action counter, the MK2 pause flag, asurabld contact,
//! asurabld `attacking`) — two of which reached WRONG conclusions because the
//! control half of the protocol was skipped or fudged. Every "honesty
//! requirement" in §6 is therefore a first-class output field here, not a
//! comment: a hunt with no control label WARNS, marks taken with the gate
//! closed are FLAGGED, zero candidates is a RESULT, and the report always
//! states the ring/PRE/POST settings it actually used.
//!
//! Shape:
//!   * [`sample`] runs once per emulated frame (wired from `main.rs`, both the
//!     Bevy chain and the headless loop) and pushes one snapshot of the hunt
//!     region into a bounded ring.
//!   * [`mark`] records a labeled moment: frame, wall-clock, gate state, and —
//!     crucially — it PINS the `mark - PRE` snapshot out of the ring right then
//!     and schedules the `mark + POST` capture. The ring is only 60 frames; a
//!     real hunt spans minutes, so marks must own their own evidence.
//!   * [`analyze`] is a PURE kernel over pinned snapshots — no emulator, no
//!     locks, no profile — which is what makes the §7 acceptance behavior
//!     unit-testable against synthetic byte sequences.
//!
//! The tool NEVER writes a profile (§5). Candidates are hypotheses; promotion
//! needs a write-test, which is a human decision.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Mutex, OnceLock};

use crate::debug::{DebugState, SharedDebugState};

// ── tunables (all reported in every analysis, per §6) ───────────────────────

/// Snapshots retained in the rolling ring (§3 default).
pub const DEFAULT_RING_FRAMES: usize = 60;
/// Frames BEFORE a mark that form the "before" side of its changed-set (§4).
pub const DEFAULT_PRE: u64 = 4;
/// Frames AFTER a mark that form the "after" side of its changed-set (§4).
pub const DEFAULT_POST: u64 = 12;

/// Budget on the ring's total footprint (region bytes × ring frames).
///
/// §3 is explicit that a silently-truncated hunt is worse than no hunt: MK2
/// arcade's exposed region is 2.3 MB and 60 frames of it is 138 MB. We refuse
/// with a message that NAMES the size rather than degrade. 8 MiB is ~20x the
/// default two-fighter-struct scope on both shipped games, so the refusal only
/// fires on genuinely unscoped requests.
pub const RING_BUDGET_BYTES: usize = 8 << 20;

/// A frame contributes to idle churn only if it is at least this far outside
/// every mark's `[frame-PRE, frame+POST]` span. Without the guard, the frames
/// in which the event is actually happening would poison the idle set and
/// disqualify the very byte being hunted.
pub const IDLE_GUARD_FRAMES: u64 = 30;

/// Cap on the pending-diff queue that defers idle-churn commits (a diff is only
/// committed once no future mark can still claim its frame).
const IDLE_COMMIT_LAG: u64 = IDLE_GUARD_FRAMES + DEFAULT_POST + 2;

/// Values at or below this count as "small" for ranking (§4).
const SMALL_VALUE: u8 = 16;

/// A single-frame diff touching more than this fraction of the region is not
/// game churn — it is a DISCONTINUITY (a save-state load, a round/scene
/// transition). Folding one into the idle set poisons it with the entire
/// region, which silently disqualifies every real candidate. Live-observed on
/// the MK2 acceptance run: reloading the arena between marks put the health
/// bytes into idle churn and zeroed the candidate list.
const DISCONTINUITY_FRACTION: usize = 4; // > 1/4 of the region


// ── region layout ───────────────────────────────────────────────────────────

/// One contiguous guest window inside the hunt region.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Window {
    /// Display name used in profile-relative addressing (`block1`, `block2`,
    /// `window`).
    pub name: String,
    pub addr: u32,
    pub len: u32,
}

/// The hunt region: an ordered list of guest windows, sampled into one flat
/// buffer. Analysis works in FLAT OFFSETS; this maps them back to guest
/// addresses and to the profile-relative names §5 requires.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Layout {
    pub windows: Vec<Window>,
}

impl Layout {
    pub fn total(&self) -> usize {
        self.windows.iter().map(|w| w.len as usize).sum()
    }

    /// Flat offset → (window index, offset within that window).
    pub fn locate(&self, off: usize) -> Option<(usize, u32)> {
        let mut base = 0usize;
        for (i, w) in self.windows.iter().enumerate() {
            let len = w.len as usize;
            if off < base + len {
                return Some((i, (off - base) as u32));
            }
            base += len;
        }
        None
    }

    /// Flat offset → absolute guest address.
    pub fn addr_of(&self, off: usize) -> Option<u32> {
        let (i, within) = self.locate(off)?;
        Some(self.windows[i].addr.wrapping_add(within))
    }

    /// The OTHER byte of the aligned 16-bit guest word containing `off`, when
    /// it lies in the same window. Used for the §4 "byte-over-word" tiebreak:
    /// a lone byte outranks one that is merely half of a 16-bit value.
    pub fn word_partner(&self, off: usize) -> Option<usize> {
        let (i, within) = self.locate(off)?;
        let addr = self.windows[i].addr.wrapping_add(within);
        // Partner in GUEST address space (word alignment is a property of the
        // machine, not of where our window happens to start).
        let partner_addr = addr ^ 1;
        let w = &self.windows[i];
        if partner_addr < w.addr || partner_addr >= w.addr.wrapping_add(w.len) {
            return None;
        }
        let base: usize = self.windows[..i].iter().map(|w| w.len as usize).sum();
        Some(base + (partner_addr - w.addr) as usize)
    }

    /// Profile-relative rendering of a flat offset (§5): `block2+0x6F`, plus
    /// the named fighter field when the profile already knows that offset.
    /// Falls back to a bare guest address when nothing better applies.
    pub fn name_offset(&self, off: usize) -> String {
        let Some((i, within)) = self.locate(off) else {
            return format!("<offset {off} out of layout>");
        };
        let w = &self.windows[i];
        let addr = w.addr.wrapping_add(within);
        if w.name == "window" {
            return format!("0x{addr:X}");
        }
        format!("{}+0x{:X}", w.name, within)
    }
}

// ── configuration ───────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub struct HuntConfig {
    pub layout: Layout,
    pub ring_frames: usize,
    pub pre: u64,
    pub post: u64,
    /// Fold the idle-churn set into the control set (§4). Default true; the
    /// analysis reports BOTH ways regardless so a reader can see exactly what
    /// idle churn cost them.
    pub include_idle: bool,
}

impl Default for HuntConfig {
    fn default() -> Self {
        HuntConfig {
            layout: Layout::default(),
            ring_frames: DEFAULT_RING_FRAMES,
            pre: DEFAULT_PRE,
            post: DEFAULT_POST,
            include_idle: true,
        }
    }
}

/// The two fighter structs from the loaded profile — the §3 default scope.
pub fn default_layout() -> Layout {
    let p = crate::profile::current();
    let stride = p.port.memory.blocks.stride.0;
    Layout {
        windows: vec![
            Window { name: "block1".into(), addr: p.block1(), len: stride },
            Window { name: "block2".into(), addr: p.block2(), len: stride },
        ],
    }
}

/// §3's refusal. Returns `Err` with a message that NAMES the size when the ring
/// footprint would exceed [`RING_BUDGET_BYTES`]. Pure so it is unit-testable
/// without an emulator.
pub fn check_budget(layout: &Layout, ring_frames: usize) -> Result<usize, String> {
    let region = layout.total();
    let footprint = region.saturating_mul(ring_frames);
    if region == 0 {
        return Err("hunt region is EMPTY (0 bytes) — nothing to sample".to_string());
    }
    if footprint > RING_BUDGET_BYTES {
        return Err(format!(
            "REFUSED: hunt region is {region} bytes ({:.2} MiB); {ring_frames} frames of it is \
             {:.1} MiB, over the {:.1} MiB ring budget. Scope the region (hunt_configure with a \
             smaller start/len, or fewer ring frames) — a silently truncated hunt produces \
             confident wrong answers, which is worse than no hunt.",
            region as f64 / (1 << 20) as f64,
            footprint as f64 / (1 << 20) as f64,
            RING_BUDGET_BYTES as f64 / (1 << 20) as f64,
        ));
    }
    Ok(footprint)
}

// ── marks ───────────────────────────────────────────────────────────────────

/// One labeled moment plus the evidence pinned around it. §2: a mark records
/// frame, wall-clock, label, and the gate state at that frame.
#[derive(Clone, Debug)]
pub struct MarkRecord {
    pub id: usize,
    pub label: String,
    pub frame: u64,
    pub wall: String,
    /// The profile's controllable gate at the marked frame. §6: marks taken
    /// with the gate CLOSED are flagged in the report — a "blocked hit" marked
    /// during a KO freeze or a menu is exactly the kind of bad evidence that
    /// produced the retracted conclusions.
    pub gate_open: bool,
    /// PRE/POST actually used for THIS mark (they are configurable, so a mark
    /// taken under different settings must not be silently mixed in).
    pub pre: u64,
    pub post: u64,
    /// Frame the pre snapshot really came from (may differ from `frame - pre`
    /// when the ring did not reach back that far — reported when it does).
    pub pre_frame: Option<u64>,
    pub pre_snapshot: Option<Vec<u8>>,
    pub post_frame: Option<u64>,
    pub post_snapshot: Option<Vec<u8>>,
    /// True when the pre snapshot had to be taken from a shallower frame than
    /// requested (ring not yet full / configured mid-session).
    pub pre_truncated: bool,
}

impl MarkRecord {
    /// A mark can only contribute to analysis once BOTH sides are pinned.
    pub fn usable(&self) -> bool {
        self.pre_snapshot.is_some() && self.post_snapshot.is_some()
    }

    /// Byte offsets that differ between this mark's pre and post snapshots.
    pub fn changed_set(&self) -> BTreeSet<usize> {
        let (Some(a), Some(b)) = (&self.pre_snapshot, &self.post_snapshot) else {
            return BTreeSet::new();
        };
        let n = a.len().min(b.len());
        (0..n).filter(|&i| a[i] != b[i]).collect()
    }
}

// ── analysis ────────────────────────────────────────────────────────────────

/// One surviving hypothesis with everything a human needs to overrule the rank.
#[derive(Clone, Debug, serde::Serialize)]
pub struct Candidate {
    pub offset: usize,
    pub addr: u32,
    /// Profile-relative name (§5): `block2+0x6F`.
    pub name: String,
    /// `Some(offset)` when this offset exists at the SAME struct offset in both
    /// fighter blocks and both survived — the "it's a per-fighter field" tell.
    pub also_in_other_block: Option<String>,
    /// Per event mark: (mark id, label-relative index, pre value, post value).
    /// §4 requires these to always be reported — the reader must be able to
    /// judge rather than trust the ranking.
    pub event_transitions: Vec<(usize, u8, u8)>,
    /// Same, for every usable control mark (all of these are non-changes by
    /// construction, but seeing them is what makes the elimination auditable).
    pub control_values: Vec<(usize, u8, u8)>,
    pub small_values: bool,
    pub counter_like: bool,
    pub byte_like: bool,
    /// Every event mark showed the identical pre→post pair (flag-like).
    pub consistent: bool,
}

/// The full, honest result of one hunt (§6). Every field here exists so a
/// reader can reconstruct what was and was not tested.
#[derive(Clone, Debug, serde::Serialize)]
pub struct Analysis {
    pub event_label: String,
    pub control_label: Option<String>,
    // settings actually used (§6: "the report states the ring/PRE/POST settings")
    pub ring_frames: usize,
    pub pre: u64,
    pub post: u64,
    pub include_idle: bool,
    pub region_bytes: usize,
    pub region_windows: Vec<String>,
    // mark bookkeeping
    pub event_marks: usize,
    pub event_marks_usable: usize,
    pub control_marks: usize,
    pub control_marks_usable: usize,
    pub gate_closed_marks: Vec<usize>,
    // set sizes
    pub event_set_size: usize,
    pub control_mark_set_size: usize,
    pub idle_churn_size: usize,
    pub eliminated_by_control_marks: usize,
    pub eliminated_by_idle_churn: usize,
    /// Candidates = event set − control set, ranked (§4).
    pub candidates: Vec<Candidate>,
    /// The same computation WITHOUT the idle-churn subtraction, so a hunt that
    /// idle churn wiped out is visibly distinguishable from one that had no
    /// signal. Names only.
    pub candidates_ignoring_idle: Vec<String>,
    /// Prominent, ordered warnings. §6's "no control label" warning is always
    /// first when it applies.
    pub warnings: Vec<String>,
    /// Plain-language verdict, including the explicit zero-candidate RESULT.
    pub verdict: String,
}

/// THE analysis kernel (§4). Pure: snapshots in, ranked candidates out — no
/// emulator, no locks, no profile. Unit-tested against synthetic sequences.
///
/// * event set  = INTERSECTION of every event mark's changed-set (a byte that
///   misses even one event is not the signal).
/// * control set = UNION of every control mark's changed-set, plus idle churn.
/// * candidates = event set − control set.
pub fn analyze(
    layout: &Layout,
    marks: &[MarkRecord],
    event_label: &str,
    control_label: Option<&str>,
    idle_churn: &BTreeSet<usize>,
    cfg: &HuntConfig,
) -> Analysis {
    let mut warnings: Vec<String> = Vec::new();

    let events: Vec<&MarkRecord> = marks.iter().filter(|m| m.label == event_label).collect();
    let usable_events: Vec<&MarkRecord> = events.iter().copied().filter(|m| m.usable()).collect();
    let controls: Vec<&MarkRecord> = match control_label {
        Some(l) => marks.iter().filter(|m| m.label == l).collect(),
        None => Vec::new(),
    };
    let usable_controls: Vec<&MarkRecord> =
        controls.iter().copied().filter(|m| m.usable()).collect();

    // §6, the loudest one: a hunt without a control is how action_counter was
    // briefly mistaken for a contact signal.
    if control_label.is_none() {
        warnings.push(
            "*** NO CONTROL LABEL. Every byte that merely moves during the event — animation \
             counters, the attacker's own action byte, timers — is still in this list. A hunt \
             without a control is how MK2's action_counter was briefly mistaken for a contact \
             signal and had to be retracted. Re-run with control marks on near-misses before \
             believing anything below. ***"
                .to_string(),
        );
    } else if usable_controls.is_empty() {
        warnings.push(format!(
            "*** control label '{}' has NO usable marks (0 of {}), so nothing was subtracted — \
             treat this exactly like a no-control hunt. ***",
            control_label.unwrap_or(""),
            controls.len()
        ));
    }

    if events.len() > usable_events.len() {
        warnings.push(format!(
            "{} of {} '{event_label}' mark(s) are unusable (the mark+POST snapshot was never \
             captured — the run ended too soon after the mark, or sampling was off). They were \
             EXCLUDED from the intersection.",
            events.len() - usable_events.len(),
            events.len()
        ));
    }
    if controls.len() > usable_controls.len() {
        warnings.push(format!(
            "{} of {} control mark(s) are unusable and were excluded from the control set.",
            controls.len() - usable_controls.len(),
            controls.len()
        ));
    }

    // §6: marks taken while the gate was closed are flagged.
    let gate_closed: Vec<usize> = marks
        .iter()
        .filter(|m| !m.gate_open && (m.label == event_label || Some(m.label.as_str()) == control_label))
        .map(|m| m.id)
        .collect();
    if !gate_closed.is_empty() {
        warnings.push(format!(
            "GATE CLOSED at mark(s) {:?} — the profile's controllable gate said 'not in a live \
             fight' at those frames. A mark taken during a KO freeze, a menu, or a round \
             transition is suspect evidence; consider hunt_reset and re-marking.",
            gate_closed
        ));
    }

    // Mixed PRE/POST across marks makes the window setting non-uniform (§6:
    // "a candidate that only appears at one window setting is suspect").
    let mixed = usable_events
        .iter()
        .chain(usable_controls.iter())
        .any(|m| m.pre != cfg.pre || m.post != cfg.post);
    if mixed {
        warnings.push(
            "Marks were taken under DIFFERENT pre/post settings than the ones reported here; \
             per-mark windows are not uniform. Reset and re-mark for a clean run."
                .to_string(),
        );
    }
    if usable_events.iter().any(|m| m.pre_truncated) {
        warnings.push(
            "At least one mark's PRE snapshot came from a shallower frame than requested (the \
             ring had not filled yet). Its changed-set spans a shorter window than the others."
                .to_string(),
        );
    }

    // ── the kernel proper ──────────────────────────────────────────────────
    let mut event_set: Option<BTreeSet<usize>> = None;
    for m in &usable_events {
        let cs = m.changed_set();
        event_set = Some(match event_set {
            None => cs,
            Some(acc) => acc.intersection(&cs).copied().collect(),
        });
    }
    let event_set = event_set.unwrap_or_default();

    let mut control_mark_set: BTreeSet<usize> = BTreeSet::new();
    for m in &usable_controls {
        control_mark_set.extend(m.changed_set());
    }

    let after_controls: BTreeSet<usize> =
        event_set.difference(&control_mark_set).copied().collect();
    let surviving: BTreeSet<usize> = if cfg.include_idle {
        after_controls.difference(idle_churn).copied().collect()
    } else {
        after_controls.clone()
    };

    // Word-partner tiebreak needs the final surviving set.
    let mut cands: Vec<Candidate> = surviving
        .iter()
        .map(|&off| {
            let event_transitions: Vec<(usize, u8, u8)> = usable_events
                .iter()
                .map(|m| {
                    (
                        m.id,
                        m.pre_snapshot.as_ref().and_then(|v| v.get(off).copied()).unwrap_or(0),
                        m.post_snapshot.as_ref().and_then(|v| v.get(off).copied()).unwrap_or(0),
                    )
                })
                .collect();
            let control_values: Vec<(usize, u8, u8)> = usable_controls
                .iter()
                .map(|m| {
                    (
                        m.id,
                        m.pre_snapshot.as_ref().and_then(|v| v.get(off).copied()).unwrap_or(0),
                        m.post_snapshot.as_ref().and_then(|v| v.get(off).copied()).unwrap_or(0),
                    )
                })
                .collect();

            let small_values = event_transitions
                .iter()
                .all(|(_, a, b)| *a < SMALL_VALUE && *b < SMALL_VALUE);
            let counter_like = is_counter_like(&event_transitions);
            let consistent = event_transitions
                .windows(2)
                .all(|w| (w[0].1, w[0].2) == (w[1].1, w[1].2));
            let byte_like = match layout.word_partner(off) {
                Some(p) => !surviving.contains(&p),
                None => true,
            };
            Candidate {
                offset: off,
                addr: layout.addr_of(off).unwrap_or(0),
                name: layout.name_offset(off),
                also_in_other_block: sibling_block_name(layout, off, &surviving),
                event_transitions,
                control_values,
                small_values,
                counter_like,
                byte_like,
                consistent,
            }
        })
        .collect();

    // §4 ranking: fires on all events (already guaranteed by the intersection),
    // then small values, then counter-like, then byte-over-word, then address
    // order for determinism.
    cands.sort_by_key(|c| {
        (
            u8::from(!c.small_values),
            u8::from(!c.counter_like),
            u8::from(!c.byte_like),
            c.offset,
        )
    });

    let verdict = if usable_events.is_empty() {
        format!(
            "NO RESULT: there are no usable '{event_label}' marks. Nothing was analyzed."
        )
    } else if cands.is_empty() {
        format!(
            "ZERO CANDIDATES — this is a RESULT, not a failure. No byte in the {} B hunt region \
             fires on all {} '{event_label}' mark(s) AND stays quiet on the control set \
             ({} control mark(s), {} idle-churn byte(s)). Either the signal lives outside the \
             scoped region, or it does not exist as a byte-level difference over a \
             -{}/+{} frame window.",
            layout.total(),
            usable_events.len(),
            usable_controls.len(),
            if cfg.include_idle { idle_churn.len() } else { 0 },
            cfg.pre,
            cfg.post,
        )
    } else {
        format!(
            "{} candidate(s) fire on all {} '{event_label}' mark(s) and on nothing in the \
             control set. These are HYPOTHESES: confirm with a write-test before putting any of \
             them in a profile.",
            cands.len(),
            usable_events.len()
        )
    };

    Analysis {
        event_label: event_label.to_string(),
        control_label: control_label.map(|s| s.to_string()),
        ring_frames: cfg.ring_frames,
        pre: cfg.pre,
        post: cfg.post,
        include_idle: cfg.include_idle,
        region_bytes: layout.total(),
        region_windows: layout
            .windows
            .iter()
            .map(|w| format!("{} 0x{:X}..0x{:X} ({} B)", w.name, w.addr, w.addr + w.len, w.len))
            .collect(),
        event_marks: events.len(),
        event_marks_usable: usable_events.len(),
        control_marks: controls.len(),
        control_marks_usable: usable_controls.len(),
        gate_closed_marks: gate_closed,
        event_set_size: event_set.len(),
        control_mark_set_size: control_mark_set.len(),
        idle_churn_size: idle_churn.len(),
        eliminated_by_control_marks: event_set.len() - after_controls.len(),
        eliminated_by_idle_churn: after_controls.len() - surviving.len(),
        candidates: cands,
        candidates_ignoring_idle: after_controls
            .iter()
            .map(|&o| layout.name_offset(o))
            .collect(),
        warnings,
        verdict,
    }
}

/// §4's "counter-like": the value ACCUMULATES across marks — a fixed non-zero
/// delta, or a monotone-non-decreasing post value that actually rises.
///
/// The across-marks variation requirement is load-bearing. Without it a plain
/// 0→1 flag repeated at every mark has "a fixed delta of +1" and would be
/// mis-ranked as a counter, which is precisely how a swing counter and a
/// contact flag got confused by hand.
fn is_counter_like(transitions: &[(usize, u8, u8)]) -> bool {
    if transitions.len() < 2 {
        return false;
    }
    let identical = transitions.windows(2).all(|w| (w[0].1, w[0].2) == (w[1].1, w[1].2));
    if identical {
        return false;
    }
    let deltas: Vec<i32> = transitions
        .iter()
        .map(|(_, a, b)| *b as i32 - *a as i32)
        .collect();
    let fixed_delta = deltas[0] != 0 && deltas.iter().all(|d| *d == deltas[0]);
    let posts: Vec<u8> = transitions.iter().map(|(_, _, b)| *b).collect();
    let monotone = posts.windows(2).all(|w| w[0] <= w[1]) && posts.first() < posts.last();
    fixed_delta || monotone
}

/// When `off` lives in one fighter block and the SAME struct offset also
/// survived in the other block, name it — the "this is a per-fighter field"
/// tell that separates a real struct member from a coincidence.
fn sibling_block_name(layout: &Layout, off: usize, surviving: &BTreeSet<usize>) -> Option<String> {
    let (i, within) = layout.locate(off)?;
    let here = &layout.windows[i];
    if here.name != "block1" && here.name != "block2" {
        return None;
    }
    let other_name = if here.name == "block1" { "block2" } else { "block1" };
    let (j, other) = layout
        .windows
        .iter()
        .enumerate()
        .find(|(_, w)| w.name == other_name)?;
    if within >= other.len {
        return None;
    }
    let base: usize = layout.windows[..j].iter().map(|w| w.len as usize).sum();
    let other_off = base + within as usize;
    surviving
        .contains(&other_off)
        .then(|| format!("{other_name}+0x{within:X}"))
}

// ── evidence-doc export (§5) ────────────────────────────────────────────────

/// Render an [`Analysis`] in the evidence-doc format used by
/// `library/<family>/<port>.md`. Explicitly says the tool did not and will not
/// write a profile.
pub fn export_markdown(a: &Analysis) -> String {
    let mut s = String::new();
    s.push_str("### Signal hunt — event-marked differential RAM analysis\n\n");
    s.push_str(&format!(
        "**Method.** Ring {} frames; changed-set per mark = diff(mark-{}, mark+{}); \
         event set = INTERSECTION over event marks; control set = UNION over control marks{}; \
         candidates = event − control. Region: {} B — {}.\n\n",
        a.ring_frames,
        a.pre,
        a.post,
        if a.include_idle { " plus idle churn" } else { " (idle churn NOT subtracted)" },
        a.region_bytes,
        a.region_windows.join(", ")
    ));
    s.push_str(&format!(
        "**Marks.** event `{}`: {} ({} usable). controls used: {}.\n\n",
        a.event_label,
        a.event_marks,
        a.event_marks_usable,
        match &a.control_label {
            Some(l) => format!("`{}` — {} ({} usable)", l, a.control_marks, a.control_marks_usable),
            None => "**NONE**".to_string(),
        }
    ));
    if !a.gate_closed_marks.is_empty() {
        s.push_str(&format!(
            "**Gate.** Marks {:?} were taken with the profile gate CLOSED.\n\n",
            a.gate_closed_marks
        ));
    }
    for w in &a.warnings {
        s.push_str(&format!("> ⚠ {w}\n>\n"));
    }
    if !a.warnings.is_empty() {
        s.push('\n');
    }
    s.push_str(&format!("**Result.** {}\n\n", a.verdict));
    s.push_str(&format!(
        "Set sizes: event {} · control-marks {} · idle churn {} → eliminated {} by controls, \
         {} by idle churn.\n\n",
        a.event_set_size,
        a.control_mark_set_size,
        a.idle_churn_size,
        a.eliminated_by_control_marks,
        a.eliminated_by_idle_churn
    ));
    if !a.candidates.is_empty() {
        s.push_str("| # | address | small | counter | byte | per-event-mark pre→post |\n");
        s.push_str("|---|---------|-------|---------|------|--------------------------|\n");
        for (i, c) in a.candidates.iter().enumerate() {
            let trans = c
                .event_transitions
                .iter()
                .map(|(id, a0, b0)| format!("m{id}: {a0}→{b0}"))
                .collect::<Vec<_>>()
                .join(", ");
            s.push_str(&format!(
                "| {} | `{}` (0x{:X}){} | {} | {} | {} | {} |\n",
                i + 1,
                c.name,
                c.addr,
                match &c.also_in_other_block {
                    Some(o) => format!(" · also `{o}`"),
                    None => String::new(),
                },
                if c.small_values { "yes" } else { "no" },
                if c.counter_like { "yes" } else { "no" },
                if c.byte_like { "yes" } else { "word" },
                trans
            ));
        }
        s.push('\n');
        if !a.candidates.is_empty() && a.control_marks_usable > 0 {
            s.push_str("Control-mark readings for the same offsets (all non-changes by \
                        construction — shown so the elimination is auditable):\n\n");
            for c in a.candidates.iter().take(8) {
                let cv = c
                    .control_values
                    .iter()
                    .map(|(id, a0, b0)| format!("m{id}: {a0}→{b0}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                s.push_str(&format!("- `{}`: {}\n", c.name, cv));
            }
            s.push('\n');
        }
    }
    s.push_str(
        "**Not a profile write.** These are hypotheses produced by differencing, nothing more. \
         Promotion into a game profile requires a WRITE-TEST (poke the byte, watch the game \
         react) — a human/agent decision this tool deliberately does not make.\n",
    );
    s
}

// ── live state (process-wide, like the profile) ─────────────────────────────

struct Snapshot {
    frame: u64,
    bytes: Vec<u8>,
}

/// The running hunt. Process-wide because every surface (MCP thread, Lua on the
/// main thread, the debugger panel, the per-frame sampler) needs the same one,
/// and none of them can pass it to the others.
pub struct HuntState {
    pub cfg: HuntConfig,
    /// False until the first sample resolves the default layout from the
    /// profile (or `configure` sets one explicitly).
    pub resolved: bool,
    /// Sampling on/off. ON by default: the PRE half of every mark comes out of
    /// the ring, so the ring must already be running BEFORE the first mark —
    /// arm-on-first-mark would silently produce pre-less marks.
    pub enabled: bool,
    ring: VecDeque<Snapshot>,
    pub marks: Vec<MarkRecord>,
    pub idle_churn: BTreeSet<usize>,
    pub idle_frames: u64,
    /// Per-frame diffs waiting out [`IDLE_COMMIT_LAG`] so a mark arriving later
    /// can still disqualify them from the idle set. The bool is whether BOTH
    /// endpoints of the diff were input-quiet frames.
    pending_idle: VecDeque<(u64, Vec<usize>, bool)>,
    /// Whether the previous sampled frame had no controller input on either
    /// port — see [`sample`] for why input quiet is part of "quiet".
    prev_input_quiet: bool,
    /// Frames whose diff was rejected as a DISCONTINUITY (save-state load,
    /// scene change). Reported so a run that reloaded a lot is visibly so.
    pub discontinuities: u64,
    pub frames_sampled: u64,
    last_frame: Option<u64>,
    next_id: usize,
    /// Sticky one-line status for the panel / tool replies (unreadable region,
    /// budget refusal, …).
    pub note: Option<String>,
    /// Last analysis, kept so the panel can render + export without re-running.
    pub last_analysis: Option<Analysis>,
}

impl HuntState {
    fn new() -> Self {
        HuntState {
            cfg: HuntConfig::default(),
            resolved: false,
            enabled: true,
            ring: VecDeque::new(),
            marks: Vec::new(),
            idle_churn: BTreeSet::new(),
            idle_frames: 0,
            pending_idle: VecDeque::new(),
            prev_input_quiet: false,
            discontinuities: 0,
            frames_sampled: 0,
            last_frame: None,
            next_id: 1,
            note: None,
            last_analysis: None,
        }
    }

    /// Clear captured evidence but keep the configuration.
    pub fn reset(&mut self) {
        self.ring.clear();
        self.marks.clear();
        self.idle_churn.clear();
        self.idle_frames = 0;
        self.pending_idle.clear();
        self.prev_input_quiet = false;
        self.discontinuities = 0;
        self.frames_sampled = 0;
        self.last_frame = None;
        self.next_id = 1;
        self.last_analysis = None;
    }

    pub fn ring_len(&self) -> usize {
        self.ring.len()
    }

    /// Live per-label mark counts for the panel.
    pub fn label_counts(&self) -> BTreeMap<String, (usize, usize)> {
        let mut m: BTreeMap<String, (usize, usize)> = BTreeMap::new();
        for mk in &self.marks {
            let e = m.entry(mk.label.clone()).or_default();
            e.0 += 1;
            if mk.usable() {
                e.1 += 1;
            }
        }
        m
    }

    /// Newest snapshot at or before `frame`, else the oldest available.
    fn snapshot_at_or_before(&self, frame: u64) -> Option<(&Snapshot, bool)> {
        let mut best: Option<&Snapshot> = None;
        for s in &self.ring {
            if s.frame <= frame {
                best = Some(s);
            }
        }
        match best {
            Some(s) => Some((s, false)),
            None => self.ring.front().map(|s| (s, true)),
        }
    }

    /// Is `frame` far enough from every mark to count as quiet idle time?
    pub fn far_from_marks(&self, frame: u64) -> bool {
        self.marks.iter().all(|m| {
            let lo = m.frame.saturating_sub(m.pre + IDLE_GUARD_FRAMES);
            let hi = m.frame + m.post + IDLE_GUARD_FRAMES;
            frame < lo || frame > hi
        })
    }
}

fn cell() -> &'static Mutex<HuntState> {
    static HUNT: OnceLock<Mutex<HuntState>> = OnceLock::new();
    HUNT.get_or_init(|| Mutex::new(HuntState::new()))
}

/// Run `f` with the process-wide hunt state. Returns `None` only if the mutex
/// is poisoned (a panicking holder), which no path here can cause.
pub fn with_state<R>(f: impl FnOnce(&mut HuntState) -> R) -> Option<R> {
    cell().lock().ok().map(|mut g| f(&mut g))
}

// ── region reads ────────────────────────────────────────────────────────────

/// Copy `len` bytes at guest `addr` out of whichever mapped region backs them,
/// in ONE bounds-checked memcpy. Deliberately not `read_addr` per byte: that
/// walks the region list for every byte, and this runs every frame.
fn read_span(ds: &DebugState, addr: u32, len: u32, out: &mut Vec<u8>) -> bool {
    let (addr, len) = (addr as usize, len as usize);
    for region in &ds.memory_regions {
        if region.host_ptr_for_addr(addr).is_none() {
            continue;
        }
        let Some(ptr) = region.safe_host_ptr(addr, len) else {
            continue;
        };
        out.extend_from_slice(unsafe { std::slice::from_raw_parts(ptr, len) });
        return true;
    }
    false
}

/// Is a one-frame diff of `changed` bytes over a `region`-byte snapshot too
/// big to be game churn? See [`DISCONTINUITY_FRACTION`].
pub fn is_discontinuity(changed: usize, region: usize) -> bool {
    region > 0 && changed > region / DISCONTINUITY_FRACTION
}

/// Snapshot the whole hunt region. `None` when any window is unbacked — a
/// PARTIAL snapshot would silently corrupt every diff, so we take none.
fn snapshot_region(ds: &DebugState, layout: &Layout) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(layout.total());
    for w in &layout.windows {
        if !read_span(ds, w.addr, w.len, &mut out) {
            return None;
        }
    }
    Some(out)
}

// ── per-frame sampler ───────────────────────────────────────────────────────

/// Push one snapshot of the hunt region into the ring, service pending
/// mark+POST captures, and accumulate idle churn. Called once per emulated
/// frame from BOTH run loops in `main.rs` (Bevy chain and headless).
///
/// Cheap by construction (§3): the default region is the two fighter structs
/// (~7 KB on asurabld, ~0.8 KB on MK2) and this is one memcpy plus one diff.
pub fn sample(shared: &SharedDebugState) {
    // Never hold the hunt lock across a DebugState lock: read everything we
    // need from DebugState first, drop it, then touch the hunt state.
    let want = match cell().lock() {
        Ok(g) => {
            if !g.enabled {
                return;
            }
            (g.resolved.then(|| g.cfg.layout.clone()), g.last_frame)
        }
        Err(_) => return,
    };
    let (layout, last_frame) = want;

    let layout = match layout {
        Some(l) => l,
        None => {
            // First sample: resolve the §3 default scope from the profile.
            let l = default_layout();
            let mut g = match cell().lock() {
                Ok(g) => g,
                Err(_) => return,
            };
            match check_budget(&l, g.cfg.ring_frames) {
                Ok(_) => {
                    g.cfg.layout = l.clone();
                    g.resolved = true;
                    l
                }
                Err(e) => {
                    g.note = Some(e);
                    g.enabled = false;
                    return;
                }
            }
        }
    };

    let (frame, bytes, input_quiet) = {
        let Ok(ds) = shared.lock() else { return };
        if Some(ds.frame_count) == last_frame {
            return; // paused / no new emulated frame
        }
        // "Quiet" means quiet in BOTH senses: far from every mark AND nobody
        // touching a controller. Frames-far-from-a-mark alone is not enough —
        // an operator walking into range, or performing an UNMARKED instance of
        // the very event being hunted, would otherwise fold the signal itself
        // into the idle-churn set and disqualify it. Requiring neutral input on
        // both ports makes the idle set what §4 is actually after: the game's
        // own background churn (health regen, idle animation, timers).
        let quiet = !ds.input_state.iter().any(|b| *b)
            && !ds.input_state2.iter().any(|b| *b)
            && !ds.peek_injected_input(0).iter().any(|b| *b)
            && !ds.peek_injected_input(1).iter().any(|b| *b);
        (ds.frame_count, snapshot_region(&ds, &layout), quiet)
    };
    let Some(bytes) = bytes else {
        if let Ok(mut g) = cell().lock() {
            g.note = Some(
                "hunt region is not backed by any mapped memory region — no snapshots are being \
                 taken (install a bus window / check --bus-map)"
                    .into(),
            );
        }
        return;
    };

    let Ok(mut g) = cell().lock() else { return };
    g.last_frame = Some(frame);
    g.frames_sampled += 1;

    // Per-frame diff against the previous snapshot, deferred until no future
    // mark can claim this frame as part of an event window.
    let both_quiet = input_quiet && g.prev_input_quiet;
    g.prev_input_quiet = input_quiet;
    if let Some(prev) = g.ring.back() {
        // Only ADJACENT frames form a per-frame diff. A gap (the GUI's
        // catch-up burst, a resume after stepping) is not one frame of churn.
        let adjacent = prev.frame + 1 == frame;
        let n = prev.bytes.len().min(bytes.len());
        let diff: Vec<usize> = (0..n).filter(|&i| prev.bytes[i] != bytes[i]).collect();
        let discontinuity = is_discontinuity(diff.len(), n);
        if discontinuity {
            g.discontinuities += 1;
        }
        if adjacent && !discontinuity {
            g.pending_idle.push_back((frame, diff, both_quiet));
        }
    }
    while g
        .pending_idle
        .front()
        .is_some_and(|(f, _, _)| f + IDLE_COMMIT_LAG < frame)
    {
        let (f, diff, quiet) = g.pending_idle.pop_front().unwrap();
        if quiet && g.far_from_marks(f) {
            g.idle_frames += 1;
            g.idle_churn.extend(diff);
        }
    }

    // Pin any mark whose +POST frame has now arrived.
    for m in g.marks.iter_mut() {
        if m.post_snapshot.is_none() && frame >= m.frame + m.post {
            m.post_frame = Some(frame);
            m.post_snapshot = Some(bytes.clone());
        }
    }

    let cap = g.cfg.ring_frames.max(1);
    g.ring.push_back(Snapshot { frame, bytes });
    while g.ring.len() > cap {
        g.ring.pop_front();
    }
}

// ── surfaces ────────────────────────────────────────────────────────────────

/// §2: record a labeled moment. Pins the `mark - PRE` snapshot out of the ring
/// immediately (the ring is 60 frames; a hunt is minutes long) and schedules
/// the `mark + POST` capture for the sampler.
pub fn mark(shared: &SharedDebugState, label: &str) -> Result<String, String> {
    let ds = shared.lock().map_err(|_| "debug state lock poisoned".to_string())?;
    mark_with(&ds, label)
}

/// [`mark`] for callers that ALREADY hold the `DebugState` lock (the debugger
/// panel gets `&mut DebugState` from the dock). Lock order in this module is
/// strictly DebugState → hunt; the sampler never holds the hunt lock while
/// taking the DebugState one, so there is no cycle.
pub fn mark_with(ds: &DebugState, label: &str) -> Result<String, String> {
    let label = label.trim();
    if label.is_empty() {
        return Err("mark label must not be empty (convention: 'event' / 'control')".into());
    }
    let gate_open = crate::gate::eval_gate(ds, crate::profile::current());
    let frame = ds.frame_count;
    let wall = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%.3f").to_string();

    let mut g = cell().lock().map_err(|_| "hunt lock poisoned".to_string())?;
    if !g.enabled {
        return Err(format!(
            "hunt sampling is OFF{} — marks would have no evidence attached",
            g.note.as_ref().map(|n| format!(" ({n})")).unwrap_or_default()
        ));
    }
    let (pre, post) = (g.cfg.pre, g.cfg.post);
    let target = frame.saturating_sub(pre);
    let (pre_frame, pre_snapshot, truncated) = match g.snapshot_at_or_before(target) {
        Some((s, fallback)) => (Some(s.frame), Some(s.bytes.clone()), fallback),
        None => (None, None, true),
    };
    let id = g.next_id;
    g.next_id += 1;
    g.marks.push(MarkRecord {
        id,
        label: label.to_string(),
        frame,
        wall: wall.clone(),
        gate_open,
        pre,
        post,
        pre_frame,
        pre_snapshot,
        post_frame: None,
        post_snapshot: None,
        pre_truncated: truncated,
    });
    let n = g.marks.iter().filter(|m| m.label == label).count();
    Ok(format!(
        "mark #{id} '{label}' at frame {frame} ({wall}); gate {}; pre from frame {}{}; \
         post due at frame {} — that is mark {n} with this label",
        if gate_open { "OPEN" } else { "CLOSED (suspect — see the report)" },
        pre_frame.map(|f| f.to_string()).unwrap_or_else(|| "n/a".into()),
        if truncated { " (RING TOO SHALLOW — pre window truncated)" } else { "" },
        frame + post
    ))
}

/// §3: set the hunt region and window settings. Changing the REGION discards
/// captured evidence (snapshots taken under a different layout are not
/// comparable and silently reusing them is exactly the class of bug this
/// feature exists to prevent); changing only ring/pre/post keeps the marks and
/// clears just the ring.
#[allow(clippy::too_many_arguments)]
pub fn configure(
    blocks: bool,
    extra: Option<(u32, u32)>,
    ring_frames: Option<usize>,
    pre: Option<u64>,
    post: Option<u64>,
    include_idle: Option<bool>,
    enabled: Option<bool>,
) -> Result<String, String> {
    let mut windows: Vec<Window> = Vec::new();
    if blocks {
        windows.extend(default_layout().windows);
    }
    if let Some((addr, len)) = extra {
        if len == 0 {
            return Err("extra window `len` must be > 0".into());
        }
        windows.push(Window { name: "window".into(), addr, len });
    }
    let layout = Layout { windows };

    let mut g = cell().lock().map_err(|_| "hunt lock poisoned".to_string())?;
    let ring = ring_frames.unwrap_or(g.cfg.ring_frames).max(2);
    check_budget(&layout, ring)?;

    let region_changed = !g.resolved || layout != g.cfg.layout;
    g.cfg.layout = layout.clone();
    g.cfg.ring_frames = ring;
    if let Some(v) = pre {
        g.cfg.pre = v;
    }
    if let Some(v) = post {
        g.cfg.post = v;
    }
    if let Some(v) = include_idle {
        g.cfg.include_idle = v;
    }
    if let Some(v) = enabled {
        g.enabled = v;
    }
    g.resolved = true;
    g.note = None;
    let dropped = g.marks.len();
    if region_changed {
        g.reset();
    } else {
        g.ring.clear();
        g.pending_idle.clear();
    }
    Ok(format!(
        "hunt region = {} B [{}]; ring {} frames ({:.1} KiB); pre {} / post {}; idle churn {}; \
         sampling {}{}",
        layout.total(),
        layout
            .windows
            .iter()
            .map(|w| format!("{} 0x{:X}+{}", w.name, w.addr, w.len))
            .collect::<Vec<_>>()
            .join(", "),
        ring,
        (layout.total() * ring) as f64 / 1024.0,
        g.cfg.pre,
        g.cfg.post,
        if g.cfg.include_idle { "subtracted" } else { "NOT subtracted" },
        if g.enabled { "on" } else { "OFF" },
        if region_changed && dropped > 0 {
            format!(" — region changed, DISCARDED {dropped} mark(s) captured under the old layout")
        } else {
            String::new()
        }
    ))
}

/// Run [`analyze`] over the live marks and remember the result for export.
pub fn run_analysis(event_label: &str, control_label: Option<&str>) -> Result<Analysis, String> {
    let mut g = cell().lock().map_err(|_| "hunt lock poisoned".to_string())?;
    let cfg = g.cfg.clone();
    let idle = g.idle_churn.clone();
    let a = analyze(&cfg.layout, &g.marks, event_label, control_label, &idle, &cfg);
    g.last_analysis = Some(a.clone());
    Ok(a)
}

/// §2: clear all marks and captured snapshots (configuration survives).
pub fn reset() -> String {
    match cell().lock() {
        Ok(mut g) => {
            let n = g.marks.len();
            g.reset();
            format!("hunt reset — {n} mark(s) and the ring discarded; configuration kept")
        }
        Err(_) => "hunt lock poisoned".to_string(),
    }
}

/// Status block for the panel and the MCP tools.
pub fn status() -> serde_json::Value {
    let Ok(g) = cell().lock() else {
        return serde_json::json!({ "error": "hunt lock poisoned" });
    };
    serde_json::json!({
        "enabled": g.enabled,
        "resolved": g.resolved,
        "region_bytes": g.cfg.layout.total(),
        "windows": g.cfg.layout.windows.iter()
            .map(|w| format!("{} 0x{:X}+{}", w.name, w.addr, w.len)).collect::<Vec<_>>(),
        "ring_frames": g.cfg.ring_frames,
        "ring_filled": g.ring_len(),
        "pre": g.cfg.pre,
        "post": g.cfg.post,
        "include_idle": g.cfg.include_idle,
        "frames_sampled": g.frames_sampled,
        "idle_frames": g.idle_frames,
        "idle_churn_bytes": g.idle_churn.len(),
        "discontinuities_skipped": g.discontinuities,
        "marks": g.label_counts().into_iter()
            .map(|(k, (n, usable))| serde_json::json!({ "label": k, "marks": n, "usable": usable }))
            .collect::<Vec<_>>(),
        // The §2 record itself, per mark: frame, wall-clock, label, gate state
        // — plus which frames its evidence actually came from, so a mark whose
        // window landed somewhere unintended is visible BEFORE the analysis.
        "mark_log": g.marks.iter().map(|m| serde_json::json!({
            "id": m.id,
            "label": m.label,
            "frame": m.frame,
            "wall": m.wall,
            "gate_open": m.gate_open,
            "pre_frame": m.pre_frame,
            "post_frame": m.post_frame,
            "pre_truncated": m.pre_truncated,
            "usable": m.usable(),
        })).collect::<Vec<_>>(),
        "note": g.note,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A two-window synthetic layout: 8 bytes at 0x100 named block1, 8 bytes at
    /// 0x200 named block2 — enough to exercise naming, word pairing and the
    /// sibling-block tell without an emulator.
    fn layout() -> Layout {
        Layout {
            windows: vec![
                Window { name: "block1".into(), addr: 0x100, len: 8 },
                Window { name: "block2".into(), addr: 0x200, len: 8 },
            ],
        }
    }

    fn mk(id: usize, label: &str, pre: &[u8], post: &[u8]) -> MarkRecord {
        MarkRecord {
            id,
            label: label.into(),
            frame: (id as u64) * 100,
            wall: "T".into(),
            gate_open: true,
            pre: DEFAULT_PRE,
            post: DEFAULT_POST,
            pre_frame: Some((id as u64) * 100 - DEFAULT_PRE),
            pre_snapshot: Some(pre.to_vec()),
            post_frame: Some((id as u64) * 100 + DEFAULT_POST),
            post_snapshot: Some(post.to_vec()),
            pre_truncated: false,
        }
    }

    fn cfg() -> HuntConfig {
        HuntConfig { layout: layout(), ..HuntConfig::default() }
    }

    /// THE headline behavior: a byte that fires on every event and on nothing
    /// else ranks first. Offset 3 flips 0→1 on all three events and never moves
    /// on the controls; offsets 0/1 are noise that also moves on the controls.
    #[test]
    fn byte_firing_on_every_event_and_no_control_ranks_first() {
        let base = [0u8; 16];
        let mut marks = Vec::new();
        for i in 1..=3 {
            let mut post = base;
            // noise that also moves on controls
            post[0] = i as u8;
            post[9] = 200;
            // the signal
            post[3] = 1;
            marks.push(mk(i, "event", &base[..], &post[..]));
        }
        for i in 4..=5 {
            let base2 = [0u8; 16];
            let mut post = base2;
            post[0] = i as u8;
            post[9] = 200;
            marks.push(mk(i, "control", &base2[..], &post[..]));
        }
        let a = analyze(&layout(), &marks, "event", Some("control"), &BTreeSet::new(), &cfg());
        assert_eq!(a.candidates.len(), 1, "only offset 3 should survive: {a:?}");
        assert_eq!(a.candidates[0].offset, 3);
        assert_eq!(a.candidates[0].name, "block1+0x3");
        assert_eq!(a.candidates[0].event_transitions.len(), 3);
        assert!(a.candidates[0].event_transitions.iter().all(|(_, p, q)| (*p, *q) == (0, 1)));
        assert!(a.candidates[0].consistent);
        assert_eq!(a.eliminated_by_control_marks, 2);
        assert!(a.warnings.iter().all(|w| !w.contains("NO CONTROL LABEL")));
    }

    /// A byte that ALSO fires on a control is eliminated. This is the MK2
    /// action-counter regression in miniature: it fires on every event AND on
    /// every whiff, so the control set must remove it.
    #[test]
    fn byte_that_also_fires_on_a_control_is_eliminated() {
        let base = [0u8; 16];
        let mut marks = Vec::new();
        for i in 1..=4 {
            let mut post = base;
            post[5] = i as u8; // "action counter" — fires on every swing
            marks.push(mk(i, "event", &base[..], &post[..]));
        }
        // Whiff control: the swing happened, the contact did not.
        let mut cpost = base;
        cpost[5] = 9;
        marks.push(mk(9, "control", &base[..], &cpost[..]));

        let a = analyze(&layout(), &marks, "event", Some("control"), &BTreeSet::new(), &cfg());
        assert!(
            a.candidates.is_empty(),
            "the action-counter byte must NOT survive whiff controls: {:?}",
            a.candidates
        );
        assert!(a.verdict.starts_with("ZERO CANDIDATES"));

        // …and without the control it WOULD have been reported — with the loud
        // §6 warning attached. That contrast is the whole point of the control.
        let a2 = analyze(&layout(), &marks, "event", None, &BTreeSet::new(), &cfg());
        assert_eq!(a2.candidates.len(), 1);
        assert_eq!(a2.candidates[0].offset, 5);
        assert!(a2.warnings[0].contains("NO CONTROL LABEL"));
    }

    /// A byte that misses even ONE event is not the signal (§4: intersection).
    #[test]
    fn byte_that_misses_one_event_is_eliminated() {
        let base = [0u8; 16];
        let mut marks = Vec::new();
        for i in 1..=4 {
            let mut post = base;
            post[2] = 1; // fires on events 1..3 but not 4
            if i == 4 {
                post[2] = 0;
            }
            post[6] = 1; // fires on all four
            marks.push(mk(i, "event", &base[..], &post[..]));
        }
        let a = analyze(&layout(), &marks, "event", Some("control"), &BTreeSet::new(), &cfg());
        let names: Vec<usize> = a.candidates.iter().map(|c| c.offset).collect();
        assert_eq!(names, vec![6], "only the byte firing on ALL events survives");
    }

    /// Idle churn disqualifies a byte that moves without any event, and the
    /// report distinguishes that elimination from a control-mark elimination.
    #[test]
    fn idle_churn_eliminates_and_is_reported_separately() {
        let base = [0u8; 16];
        let mut marks = Vec::new();
        for i in 1..=3 {
            let mut post = base;
            post[4] = 1; // real signal
            post[12] = 7; // an animation byte that also ticks while idle
            marks.push(mk(i, "event", &base[..], &post[..]));
        }
        let idle: BTreeSet<usize> = [12usize].into_iter().collect();
        let a = analyze(&layout(), &marks, "event", None, &idle, &cfg());
        assert_eq!(a.candidates.iter().map(|c| c.offset).collect::<Vec<_>>(), vec![4]);
        assert_eq!(a.eliminated_by_idle_churn, 1);
        assert_eq!(a.eliminated_by_control_marks, 0);
        // The "what idle churn cost you" view still lists it.
        assert!(a.candidates_ignoring_idle.contains(&"block2+0x4".to_string()));
    }

    /// Zero surviving candidates is an explicit, plainly-worded RESULT (§6) —
    /// not an empty list a reader can mistake for "the tool didn't run".
    #[test]
    fn zero_candidates_is_an_explicit_result() {
        let base = [0u8; 16];
        let marks = vec![
            mk(1, "event", &base[..], &{ let mut p = base; p[1] = 1; p }[..]),
            mk(2, "event", &base[..], &{ let mut p = base; p[2] = 1; p }[..]),
        ];
        let a = analyze(&layout(), &marks, "event", None, &BTreeSet::new(), &cfg());
        assert!(a.candidates.is_empty());
        assert_eq!(a.event_set_size, 0);
        assert!(a.verdict.contains("ZERO CANDIDATES"));
        assert!(a.verdict.contains("RESULT, not a failure"));
        assert!(export_markdown(&a).contains("ZERO CANDIDATES"));
    }

    /// §3's refusal fires on an unscoped region and NAMES the size, rather than
    /// silently truncating. MK2 arcade's 2.3 MB region × 60 frames = 138 MB.
    #[test]
    fn oversized_region_is_refused_by_name_not_truncated() {
        let big = Layout {
            windows: vec![Window { name: "window".into(), addr: 0, len: 2_300_000 }],
        };
        let e = check_budget(&big, DEFAULT_RING_FRAMES).unwrap_err();
        assert!(e.contains("REFUSED"), "{e}");
        assert!(e.contains("2300000 bytes"), "message must name the size: {e}");
        assert!(e.contains("131.6 MiB"), "message must name the footprint: {e}");
        // The default two-struct scope is comfortably inside the budget.
        let ok = Layout {
            windows: vec![
                Window { name: "block1".into(), addr: 0x403798, len: 0xDB4 },
                Window { name: "block2".into(), addr: 0x40454C, len: 0xDB4 },
            ],
        };
        assert!(check_budget(&ok, DEFAULT_RING_FRAMES).is_ok());
        assert!(check_budget(&Layout::default(), 60).is_err(), "empty region is refused too");
    }

    /// Ranking order: small values, then counter-like, then byte-over-word.
    #[test]
    fn ranking_prefers_small_then_counter_then_byte() {
        let base = [0u8; 16];
        let mut marks = Vec::new();
        for i in 1..=3 {
            let mut post = base;
            post[0] = 200 + i as u8; // large values -> ranked last
            post[2] = i as u8; // small AND counter-like
            post[4] = 1; // small, flag-like (not counter-like)
            post[5] = 1; // small, and pairs with 4 as a word -> loses byte tiebreak
            marks.push(mk(i, "event", &base[..], &post[..]));
        }
        let a = analyze(&layout(), &marks, "event", None, &BTreeSet::new(), &cfg());
        let order: Vec<usize> = a.candidates.iter().map(|c| c.offset).collect();
        assert_eq!(order, vec![2, 4, 5, 0], "{:?}", a.candidates);
        assert!(a.candidates[0].counter_like);
        assert!(!a.candidates[1].counter_like);
        // 0x104/0x105 form an aligned guest word, so neither is "byte-like".
        assert!(!a.candidates[1].byte_like);
        assert!(!a.candidates[3].small_values);
    }

    /// A mark whose +POST snapshot never arrived is EXCLUDED (not treated as an
    /// all-zero snapshot, which would blow away the intersection silently) and
    /// the exclusion is warned about.
    #[test]
    fn unusable_marks_are_excluded_and_warned() {
        let base = [0u8; 16];
        let mut good = mk(1, "event", &base[..], &{ let mut p = base; p[3] = 1; p }[..]);
        good.id = 1;
        let mut pending = mk(2, "event", &base[..], &base[..]);
        pending.post_snapshot = None;
        pending.post_frame = None;
        let a = analyze(&layout(), &[good, pending], "event", None, &BTreeSet::new(), &cfg());
        assert_eq!(a.event_marks, 2);
        assert_eq!(a.event_marks_usable, 1);
        assert_eq!(a.candidates.iter().map(|c| c.offset).collect::<Vec<_>>(), vec![3]);
        assert!(a.warnings.iter().any(|w| w.contains("unusable")));
    }

    /// §6: gate-closed marks are flagged by id in the report and the export.
    #[test]
    fn gate_closed_marks_are_flagged() {
        let base = [0u8; 16];
        let mut m1 = mk(1, "event", &base[..], &{ let mut p = base; p[3] = 1; p }[..]);
        m1.gate_open = false;
        let m2 = mk(2, "event", &base[..], &{ let mut p = base; p[3] = 1; p }[..]);
        let a = analyze(&layout(), &[m1, m2], "event", None, &BTreeSet::new(), &cfg());
        assert_eq!(a.gate_closed_marks, vec![1]);
        assert!(a.warnings.iter().any(|w| w.contains("GATE CLOSED")));
        assert!(export_markdown(&a).contains("gate CLOSED"));
    }

    /// Profile-relative naming (§5) and the both-blocks tell.
    #[test]
    fn naming_is_profile_relative_and_notices_both_blocks() {
        let base = [0u8; 16];
        let marks: Vec<MarkRecord> = (1..=2)
            .map(|i| {
                let mut post = base;
                post[6] = 1; // block1+0x6
                post[14] = 1; // block2+0x6 — same struct offset
                mk(i, "event", &base[..], &post[..])
            })
            .collect();
        let a = analyze(&layout(), &marks, "event", None, &BTreeSet::new(), &cfg());
        let names: Vec<&str> = a.candidates.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["block1+0x6", "block2+0x6"]);
        assert_eq!(a.candidates[0].also_in_other_block.as_deref(), Some("block2+0x6"));
        assert_eq!(a.candidates[0].addr, 0x106);
        assert_eq!(a.candidates[1].addr, 0x206);
    }

    /// The export is the evidence-doc format (§5) and always states that the
    /// tool did not write a profile.
    #[test]
    fn export_states_method_controls_and_no_profile_write() {
        let base = [0u8; 16];
        let marks = vec![
            mk(1, "event", &base[..], &{ let mut p = base; p[3] = 1; p }[..]),
            mk(2, "event", &base[..], &{ let mut p = base; p[3] = 1; p }[..]),
            mk(3, "whiff", &base[..], &base[..]),
        ];
        let a = analyze(&layout(), &marks, "event", Some("whiff"), &BTreeSet::new(), &cfg());
        let md = export_markdown(&a);
        assert!(md.contains("controls used: `whiff`"));
        assert!(md.contains("Ring 60 frames"));
        assert!(md.contains("mark-4, mark+12"));
        assert!(md.contains("block1+0x3"));
        assert!(md.contains("Not a profile write"));
    }

    /// Frames inside a mark's guarded span are NOT idle. Without this the
    /// frames in which the event is actually happening would be folded into the
    /// idle-churn set and would disqualify the very byte being hunted.
    #[test]
    fn frames_near_a_mark_are_not_idle() {
        let base = [0u8; 16];
        let mut st = HuntState::new();
        let mut m = mk(1, "event", &base[..], &base[..]);
        m.frame = 1000;
        st.marks.push(m);
        // mark 1000, pre 4, post 12, guard 30 → [966, 1042] is not idle.
        assert!(!st.far_from_marks(1000));
        assert!(!st.far_from_marks(966));
        assert!(!st.far_from_marks(1042));
        assert!(st.far_from_marks(965));
        assert!(st.far_from_marks(1043));
        // No marks at all → every frame is far from a mark.
        st.reset();
        assert!(st.far_from_marks(1000));
    }

    /// A save-state load rewrites the whole region in one "frame". Folding
    /// that into idle churn disqualifies everything — live-observed on the MK2
    /// acceptance run (reloading the arena between marks buried the health
    /// bytes in idle churn and produced a spurious zero-candidate result).
    #[test]
    fn whole_region_rewrites_are_discontinuities_not_churn() {
        assert!(is_discontinuity(1780, 1780), "a full-region rewrite");
        assert!(is_discontinuity(600, 1780), "a third of the region");
        assert!(!is_discontinuity(445, 1780), "exactly a quarter is still churn");
        assert!(!is_discontinuity(30, 1780), "ordinary per-frame churn");
        assert!(!is_discontinuity(0, 1780));
        assert!(!is_discontinuity(0, 0), "an empty region can't be discontinuous");
    }

    /// The layout's flat-offset ↔ guest-address mapping, including the word
    /// partner that only pairs within a window.
    #[test]
    fn layout_maps_offsets_and_word_partners() {
        let l = layout();
        assert_eq!(l.total(), 16);
        assert_eq!(l.addr_of(0), Some(0x100));
        assert_eq!(l.addr_of(8), Some(0x200));
        assert_eq!(l.addr_of(16), None);
        assert_eq!(l.word_partner(0), Some(1));
        assert_eq!(l.word_partner(1), Some(0));
        // Last byte of block1 (0x107) pairs with 0x106, still inside block1.
        assert_eq!(l.word_partner(7), Some(6));
        // A window starting on an odd address has an unpaired first byte.
        let odd = Layout { windows: vec![Window { name: "window".into(), addr: 0x101, len: 2 }] };
        assert_eq!(odd.word_partner(0), None);
        assert_eq!(odd.name_offset(0), "0x101");
    }
}
