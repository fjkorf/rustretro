//! 📜 Input Log — task A1: a frame-counted input history for both controller
//! ports, the feature every serious fighting-game training mode ships (NRS,
//! SF6, Guilty Gear) and this frontend lacked.
//!
//! State lives behind its own process-wide `OnceLock<Mutex<InputLogState>>`,
//! exactly like `src/hunt.rs` — a second, unrelated in-flight edit owns
//! `src/debug/mod.rs`/`src/training.rs`/`src/debug/panels/training.rs`, so
//! this module must not touch `DebugState`'s shape. Lock-ordering discipline
//! also matches `hunt.rs`: every read out of `DebugState` happens under its
//! own lock scope, which is dropped BEFORE this module's own lock is taken —
//! never the two nested — so there is no cycle with anything else that might
//! (now or later) take these locks in the other order.
//!
//! ## What gets sampled
//! `frame_count`/`input_state`/`input_state2` are read straight off
//! `DebugState`. Those three fields are written together, once per emulated
//! frame, at the top of `Frontend::run_frame` — from
//! `self.callback_context.input_state`/`input_state2`, i.e. AFTER keyboard +
//! gamepad + MCP-injected + held (dummy) input have already been OR'd
//! together into the bits actually handed to the core (see the `bits`/
//! `bits2` fold in `main.rs`'s input-handling system, and the headless
//! loop's `take_injected_input`/`take_injected_input2` fold). So this panel
//! logs what the GAME saw, not just the keyboard — requirement (4).
//!
//! Frame numbers are `DebugState::frame_count`, never a wall-clock timestamp
//! (`docs/frames.md` §2.4: wall-clock is never a unit in this project).
//!
//! ## Pause / resume
//! `Frontend::run_frame` refreshes `frame_count`/`input_state` even on a call
//! that ends up paused (the check happens before the pause early-return), so
//! a paused session calling `run_frame` repeatedly would otherwise re-offer
//! the SAME frame number every tick. [`sample`] dedupes exactly like
//! `hunt::sample`: it no-ops whenever `frame_count` hasn't advanced since the
//! last accepted sample, so pause/resume can never push duplicate or
//! out-of-order frames into the ring.
//!
//! ## Save-state loads
//! A state load does not rewind `Frontend::frame_count` (it's a plain Rust
//! counter, untouched by `retro_unserialize`), so frame numbers alone can
//! never go backwards or repeat across a load — there is no way for this
//! module to literally interleave frames from before and after one. But a
//! load DOES jump the game's in-game context arbitrarily far in time, and
//! this panel's whole value proposition is neighboring-frame GAP counts
//! (`RunEvent::gap_before`) — a gap spanning a load boundary would silently
//! report a bogus frame count for two unrelated moments. We choose to CLEAR
//! the ring on every detected load (§7 of the task): a fresh log starting at
//! the reload is more honest than a technically-monotonic one whose numbers
//! don't mean what the reader assumes. Detection reads `DebugState::state_note`
//! (the sticky, never-consumed one-liner `Frontend::drain_state_op` publishes
//! for every state op) and fingerprints it with a cheap FNV hash to notice a
//! NEW "loaded …" note without ever cloning/allocating the string — this
//! runs every sampled frame, so it has to stay allocation-free.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use bevy_egui::egui;

use crate::debug::{DebugState, SharedDebugState};

/// RETRO_DEVICE_ID_JOYPAD raw names, in bit-index order — same fallback
/// convention `input_monitor.rs` uses (core descriptor if the game/core sent
/// one via `RETRO_ENVIRONMENT_SET_INPUT_DESCRIPTORS`, else this raw name).
const RETRO_NAMES: [&str; 12] =
    ["B", "Y", "Select", "Start", "Up", "Down", "Left", "Right", "A", "X", "L", "R"];

/// Ring capacity in FRAMES (not bytes, not wall-clock) — bounded memory,
/// requirement (5). 3600 frames is ~60s at 60fps: long enough to read back a
/// whole training rep without unbounded growth. Displayed in the panel header
/// so the capacity is never a mystery to whoever's reading the log.
pub const RING_CAPACITY_FRAMES: usize = 3600;

// ── sampler state ───────────────────────────────────────────────────────────

/// One frame's already-folded button masks for both ports (RETRO_DEVICE_ID
/// order, low 12 bits — `crate::record::pack_mask`'s layout).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Sample {
    frame: u64,
    p0: u16,
    p1: u16,
}

struct InputLogState {
    ring: VecDeque<Sample>,
    /// Last frame accepted into the ring — the pause/resume dedup key,
    /// mirroring `hunt::HuntState::last_frame`.
    last_frame: Option<u64>,
}

impl InputLogState {
    fn new() -> Self {
        InputLogState { ring: VecDeque::with_capacity(RING_CAPACITY_FRAMES), last_frame: None }
    }
}

fn cell() -> &'static Mutex<InputLogState> {
    static CELL: OnceLock<Mutex<InputLogState>> = OnceLock::new();
    CELL.get_or_init(|| Mutex::new(InputLogState::new()))
}

/// Run `f` with the process-wide input-log state. `None` only on a poisoned
/// mutex (a panicking prior holder), which no path here can cause.
fn with_state<R>(f: impl FnOnce(&mut InputLogState) -> R) -> Option<R> {
    cell().lock().ok().map(|mut g| f(&mut g))
}

/// The clear/reset control's action: discard the ring, keep sampling.
pub fn clear() {
    if let Ok(mut g) = cell().lock() {
        g.ring.clear();
        g.last_frame = None;
    }
}

// FNV-1a over the sticky `state_note` string, so a NEW state-load can be
// noticed without ever cloning or allocating (the sampler runs every frame).
// The offset basis IS `fnv1a(b"")`, which matches `state_note`'s initial
// `None` (read as `""`) — so "no note observed yet" needs no separate
// sentinel.
const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h = FNV_OFFSET_BASIS;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(FNV_PRIME);
    }
    h
}

static LAST_NOTE_FP: AtomicU64 = AtomicU64::new(FNV_OFFSET_BASIS);

/// Sample one frame's already-folded input into the ring. Call once per
/// emulated frame, right alongside `hunt::sample` (see the two call sites in
/// `main.rs`: the headless loop and the `Update`-schedule Bevy system).
///
/// Cheap by construction: one lock, two field reads, one hash of a short
/// string, no heap allocation — this runs every frame (requirement 6).
pub fn sample(shared: &SharedDebugState) {
    let (frame, p0, p1, is_new_load) = {
        let Ok(ds) = shared.lock() else { return };
        let note = ds.state_note.as_deref().unwrap_or("");
        let fp = fnv1a(note.as_bytes());
        let prev_fp = LAST_NOTE_FP.swap(fp, Ordering::Relaxed);
        let is_new_load = prev_fp != fp && note.starts_with("loaded");
        (
            ds.frame_count,
            crate::record::pack_mask(&ds.input_state),
            crate::record::pack_mask(&ds.input_state2),
            is_new_load,
        )
    };

    let Ok(mut g) = cell().lock() else { return };
    if is_new_load {
        // §7: clear rather than risk a gap silently spanning the reload.
        g.ring.clear();
        g.last_frame = None;
    }
    if Some(frame) == g.last_frame {
        return; // paused / no new emulated frame this call
    }
    g.last_frame = Some(frame);
    if g.ring.len() >= RING_CAPACITY_FRAMES {
        g.ring.pop_front();
    }
    g.ring.push_back(Sample { frame, p0, p1 });
}

// ── event-collapsed rendering ────────────────────────────────────────────────

/// One press-to-release run on a single (port, button) bit stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunEvent {
    pub start_frame: u64,
    /// Consecutive frames the button was down, INCLUSIVE of `start_frame`.
    /// A 7-frame hold is one `RunEvent` with `held_frames == 7`, never seven.
    pub held_frames: u64,
    /// Frames between this stream's PREVIOUS release and this press. `None`
    /// for the first run ever observed on this stream (nothing to gap from).
    pub gap_before: Option<u64>,
    /// Still held on the last sampled frame (no release seen yet).
    pub open: bool,
}

/// Collapse a chronological `(frame, pressed)` stream for ONE button into
/// runs. A released-then-repressed button always yields TWO `RunEvent`s, no
/// matter how short the gap between them — there is no length-based merging
/// that could misfire on a fast re-arm.
///
/// A non-contiguous frame jump between two samples (should not happen in
/// practice — `sample`'s dedup keeps the ring gap-free — but tests and any
/// future caller may feed a synthetic stream) is treated as an implicit
/// release: a run never silently spans a hole in the data it didn't observe.
pub fn collapse_runs(samples: &[(u64, bool)]) -> Vec<RunEvent> {
    let mut out = Vec::new();
    let mut run: Option<(u64, u64)> = None; // (start_frame, last_frame_in_run)
    let mut last_release: Option<u64> = None; // frame the previous run went low
    let mut prev_frame: Option<u64> = None;

    let close = |run: &mut Option<(u64, u64)>, last_release: &mut Option<u64>, release_frame: u64, out: &mut Vec<RunEvent>| {
        if let Some((start, last)) = run.take() {
            out.push(RunEvent {
                start_frame: start,
                held_frames: last - start + 1,
                gap_before: last_release.map(|r| start - r),
                open: false,
            });
            *last_release = Some(release_frame);
        }
    };

    for &(frame, pressed) in samples {
        let contiguous = prev_frame.is_none_or(|p| frame == p + 1);
        if !contiguous {
            if let Some((_, last)) = run {
                close(&mut run, &mut last_release, last + 1, &mut out);
            }
        }
        match (pressed, run) {
            (true, None) => run = Some((frame, frame)),
            (true, Some((start, _))) => run = Some((start, frame)),
            (false, Some(_)) => close(&mut run, &mut last_release, frame, &mut out),
            (false, None) => {}
        }
        prev_frame = Some(frame);
    }
    if let Some((start, last)) = run {
        out.push(RunEvent {
            start_frame: start,
            held_frames: last - start + 1,
            gap_before: last_release.map(|r| start - r),
            open: true,
        });
    }
    out
}

/// Extract one button's chronological `(frame, pressed)` stream for `port`
/// (0/1) out of the ring.
fn bit_stream(ring: &VecDeque<Sample>, port: usize, bit: usize) -> Vec<(u64, bool)> {
    ring.iter()
        .map(|s| {
            let mask = if port == 0 { s.p0 } else { s.p1 };
            (s.frame, mask & (1 << bit) != 0)
        })
        .collect()
}

/// All 12 buttons' runs for `port`, merged into one chronological (oldest
/// first) log — the interleaved view a player actually reads.
fn port_events(ring: &VecDeque<Sample>, port: usize) -> Vec<(usize, RunEvent)> {
    let mut all: Vec<(usize, RunEvent)> = Vec::new();
    for bit in 0..12 {
        for ev in collapse_runs(&bit_stream(ring, port, bit)) {
            all.push((bit, ev));
        }
    }
    all.sort_by_key(|(_, ev)| ev.start_frame);
    all
}

fn format_entry(label: &str, ev: &RunEvent) -> String {
    let held = if ev.open {
        format!("held {}f…", ev.held_frames)
    } else {
        format!("held {}f", ev.held_frames)
    };
    match ev.gap_before {
        Some(g) => format!("f{:<6} +{:<10} ({held})  gap {g}f", ev.start_frame, label),
        None => format!("f{:<6} +{:<10} ({held})", ev.start_frame, label),
    }
}

// ── panel ────────────────────────────────────────────────────────────────────

pub struct InputLogPanel {
    /// Stops the per-port ScrollArea auto-scrolling to the newest entry, so a
    /// user can scroll back through the log without it fighting them.
    freeze_scroll: bool,
}

impl InputLogPanel {
    pub fn new() -> Self {
        InputLogPanel { freeze_scroll: false }
    }

    fn label(descriptors: &[[Option<String>; 12]; 2], port: usize, i: usize) -> String {
        descriptors[port][i].clone().unwrap_or_else(|| RETRO_NAMES[i].to_string())
    }

    pub fn show(&mut self, ui: &mut egui::Ui, state: &Arc<Mutex<DebugState>>) {
        let Ok(ds) = state.lock() else {
            ui.label("Error: Could not acquire debug state lock");
            return;
        };
        let descriptors = ds.input_descriptors.clone();
        let frame_now = ds.frame_count;
        drop(ds);

        let (ring_len, p0_events, p1_events) = with_state(|st| {
            (st.ring.len(), port_events(&st.ring, 0), port_events(&st.ring, 1))
        })
        .unwrap_or((0, Vec::new(), Vec::new()));

        ui.heading("📜 Input Log");
        ui.label(
            egui::RichText::new(format!(
                "ring {ring_len}/{RING_CAPACITY_FRAMES} frames · frame-exact, post-fold (keyboard + \
                 pad + MCP-injected + training dummy) · now @ frame {frame_now}"
            ))
            .small()
            .color(egui::Color32::DARK_GRAY),
        );
        ui.label(
            egui::RichText::new(
                "A save-state load clears this ring (a gap spanning the reload would be a bogus \
                 frame count) — see the module doc in input_log.rs.",
            )
            .small()
            .color(egui::Color32::DARK_GRAY),
        );

        ui.horizontal(|ui| {
            if ui.button("Clear").on_hover_text("Discard the log ring").clicked() {
                clear();
            }
            ui.checkbox(&mut self.freeze_scroll, "Freeze scroll")
                .on_hover_text("Stop auto-scrolling to the newest entry so you can read back");
        });
        ui.separator();

        ui.columns(2, |cols| {
            Self::render_port(&mut cols[0], "P1", &descriptors, 0, &p0_events, self.freeze_scroll);
            Self::render_port(&mut cols[1], "P2", &descriptors, 1, &p1_events, self.freeze_scroll);
        });
    }

    fn render_port(
        ui: &mut egui::Ui,
        title: &str,
        descriptors: &[[Option<String>; 12]; 2],
        port: usize,
        events: &[(usize, RunEvent)],
        freeze: bool,
    ) {
        ui.label(egui::RichText::new(title).strong());
        egui::ScrollArea::vertical()
            .id_salt(("input_log_scroll", port))
            .stick_to_bottom(!freeze)
            .max_height(320.0)
            .show(ui, |ui| {
                if events.is_empty() {
                    ui.label(
                        egui::RichText::new("no input yet").small().color(egui::Color32::DARK_GRAY),
                    );
                }
                for (bit, ev) in events {
                    let label = Self::label(descriptors, port, *bit);
                    ui.label(egui::RichText::new(format_entry(&label, ev)).monospace().small());
                }
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `cell()`/`LAST_NOTE_FP` are process-wide statics — the whole point of
    /// this module (the sampler and the panel share one ring without a
    /// reference to pass between them). `cargo test` runs tests in parallel
    /// THREADS in one process, so any test that touches `sample`/`with_state`/
    /// `clear` directly must serialize against every other such test or they
    /// race each other's ring. The pure `collapse_runs` tests don't touch
    /// global state and need no guard.
    static TEST_LOCK: Mutex<()> = Mutex::new(());
    fn test_guard() -> std::sync::MutexGuard<'static, ()> {
        TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    // ── collapse_runs ────────────────────────────────────────────────────────

    #[test]
    fn seven_frame_hold_is_one_entry_not_seven() {
        let mut samples = vec![(0u64, false); 10];
        for f in 10..=16u64 {
            samples.push((f, true));
        }
        for f in 17..=20u64 {
            samples.push((f, false));
        }
        // Fix up the frame numbers (the vec! above gave every entry frame 0).
        let samples: Vec<(u64, bool)> =
            samples.iter().enumerate().map(|(i, (_, p))| (i as u64, *p)).collect();

        let events = collapse_runs(&samples);
        assert_eq!(events.len(), 1, "expected one collapsed entry, got {events:?}");
        assert_eq!(events[0].start_frame, 10);
        assert_eq!(events[0].held_frames, 7);
        assert!(!events[0].open);
        assert_eq!(events[0].gap_before, None, "first-ever run has no prior release to gap from");
    }

    #[test]
    fn release_then_repress_is_two_entries() {
        // down 10-12, up 13-14, down 15-16, up 17+ — a fast re-arm that a
        // length/pattern-based macro matcher could wrongly fuse into one.
        let pressed_frames: [(u64, bool); 8] = [
            (10, true), (11, true), (12, true),
            (13, false), (14, false),
            (15, true), (16, true),
            (17, false),
        ];
        let events = collapse_runs(&pressed_frames);
        assert_eq!(events.len(), 2, "release-then-repress must be TWO entries: {events:?}");
        assert_eq!(events[0].start_frame, 10);
        assert_eq!(events[0].held_frames, 3);
        assert_eq!(events[1].start_frame, 15);
        assert_eq!(events[1].held_frames, 2);
    }

    #[test]
    fn gap_is_measured_between_the_previous_release_and_this_press() {
        let samples: [(u64, bool); 8] = [
            (10, true), (11, true), (12, true), // run 1: 10..=12, released at 13
            (13, false), (14, false),
            (15, true), (16, true), // run 2 starts at 15
            (17, false),
        ];
        let events = collapse_runs(&samples);
        assert_eq!(events.len(), 2);
        // Run 1 released at frame 13 (the first low frame); run 2 presses at
        // frame 15 — a 2-frame gap.
        assert_eq!(events[1].gap_before, Some(2));
    }

    #[test]
    fn still_held_run_is_marked_open_with_no_release() {
        let samples: [(u64, bool); 5] = [(0, false), (1, true), (2, true), (3, true), (4, true)];
        let events = collapse_runs(&samples);
        assert_eq!(events.len(), 1);
        assert!(events[0].open);
        assert_eq!(events[0].held_frames, 4);
    }

    #[test]
    fn never_pressed_stream_yields_no_events() {
        let samples: Vec<(u64, bool)> = (0..30).map(|f| (f, false)).collect();
        assert!(collapse_runs(&samples).is_empty());
    }

    #[test]
    fn non_contiguous_frame_jump_closes_the_run_at_the_gap() {
        // Held from frame 5, but the stream jumps straight to frame 50 still
        // "true" — we never observed frames 6..49, so the run must not be
        // reported as a single 46-frame hold.
        let samples: [(u64, bool); 3] = [(5, true), (6, true), (50, true)];
        let events = collapse_runs(&samples);
        assert_eq!(events.len(), 2, "a frame-number gap must not silently span a run: {events:?}");
        assert_eq!(events[0].start_frame, 5);
        assert_eq!(events[0].held_frames, 2);
        assert!(!events[0].open);
        assert_eq!(events[1].start_frame, 50);
        assert!(events[1].open);
    }

    // ── ring wraparound ──────────────────────────────────────────────────────

    #[test]
    fn ring_drops_oldest_past_capacity() {
        let _g = test_guard();
        clear();
        for f in 0..(RING_CAPACITY_FRAMES as u64 + 25) {
            with_state(|st| {
                if st.ring.len() >= RING_CAPACITY_FRAMES {
                    st.ring.pop_front();
                }
                st.ring.push_back(Sample { frame: f, p0: 0, p1: 0 });
            });
        }
        with_state(|st| {
            assert_eq!(st.ring.len(), RING_CAPACITY_FRAMES);
            assert_eq!(st.ring.front().unwrap().frame, 25, "oldest 25 frames should have fallen off");
            assert_eq!(st.ring.back().unwrap().frame, RING_CAPACITY_FRAMES as u64 + 24);
        });
        clear();
    }

    #[test]
    fn clear_empties_the_ring_and_resets_dedup() {
        let _g = test_guard();
        with_state(|st| st.ring.push_back(Sample { frame: 1, p0: 1, p1: 0 }));
        with_state(|st| st.last_frame = Some(1));
        clear();
        with_state(|st| {
            assert!(st.ring.is_empty());
            assert_eq!(st.last_frame, None);
        });
    }

    // ── sample() dedup + fold source (integration-ish, via DebugState) ──────

    #[test]
    fn sample_dedupes_unchanged_frame_count() {
        let _g = test_guard();
        crate::profile::init_for_tests();
        clear();
        let shared: SharedDebugState = Arc::new(Mutex::new(DebugState::new()));
        {
            let mut ds = shared.lock().unwrap();
            ds.frame_count = 5;
            ds.input_state[0] = true; // A held
        }
        sample(&shared);
        sample(&shared); // same frame_count — must not double-push
        with_state(|st| assert_eq!(st.ring.len(), 1));

        {
            let mut ds = shared.lock().unwrap();
            ds.frame_count = 6;
        }
        sample(&shared);
        with_state(|st| assert_eq!(st.ring.len(), 2));
        clear();
    }

    #[test]
    fn sample_reads_the_post_fold_state_not_raw_keyboard() {
        // input_state / input_state2 ARE the post-fold fields `push_input`
        // writes from `callback_context.input_state` — requirement (4). This
        // just pins that `sample()` reads exactly those fields.
        let _g = test_guard();
        crate::profile::init_for_tests();
        clear();
        let shared: SharedDebugState = Arc::new(Mutex::new(DebugState::new()));
        {
            let mut ds = shared.lock().unwrap();
            ds.frame_count = 1;
            ds.input_state[8] = true; // P1 A (bit 8)
            ds.input_state2[0] = true; // P2 B (bit 0) — e.g. dummy/shadow fold
        }
        sample(&shared);
        with_state(|st| {
            let s = *st.ring.back().unwrap();
            assert_eq!(s.p0, 1 << 8);
            assert_eq!(s.p1, 1 << 0);
        });
        clear();
    }

    #[test]
    fn detected_state_load_clears_the_ring() {
        let _g = test_guard();
        crate::profile::init_for_tests();
        clear();
        let shared: SharedDebugState = Arc::new(Mutex::new(DebugState::new()));
        {
            let mut ds = shared.lock().unwrap();
            ds.frame_count = 1;
        }
        sample(&shared);
        with_state(|st| assert_eq!(st.ring.len(), 1));

        {
            let mut ds = shared.lock().unwrap();
            ds.frame_count = 2;
            ds.state_note = Some("loaded shadow/arenas/current.state (12345 bytes) @ frame 900".into());
        }
        sample(&shared);
        // The load wipes the pre-load history; only the post-load frame remains.
        with_state(|st| {
            assert_eq!(st.ring.len(), 1);
            assert_eq!(st.ring.back().unwrap().frame, 2);
        });
        clear();
    }

    #[test]
    fn a_save_note_does_not_clear_the_ring() {
        let _g = test_guard();
        crate::profile::init_for_tests();
        clear();
        let shared: SharedDebugState = Arc::new(Mutex::new(DebugState::new()));
        {
            let mut ds = shared.lock().unwrap();
            ds.frame_count = 1;
        }
        sample(&shared);
        {
            let mut ds = shared.lock().unwrap();
            ds.frame_count = 2;
            ds.state_note = Some("saved shadow/arenas/current.state (12345 bytes) @ frame 900".into());
        }
        sample(&shared);
        with_state(|st| assert_eq!(st.ring.len(), 2, "a SAVE (not a load) must not clear the log"));
        clear();
    }

    // ── panel smoke render ───────────────────────────────────────────────────

    #[test]
    fn panel_renders_headlessly_with_and_without_history() {
        let _g = test_guard();
        crate::profile::init_for_tests();
        clear();
        let ctx = egui::Context::default();
        let mut panel = InputLogPanel::new();
        let state: Arc<Mutex<DebugState>> = Arc::new(Mutex::new(DebugState::new()));

        let draw = |panel: &mut InputLogPanel, state: &Arc<Mutex<DebugState>>| {
            let _ = ctx.run(Default::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| panel.show(ui, state));
            });
        };

        draw(&mut panel, &state); // cold: empty ring

        for f in 0..20u64 {
            {
                let mut ds = state.lock().unwrap();
                ds.frame_count = f;
                ds.input_state[8] = f % 3 == 0;
                ds.input_state2[9] = f % 5 == 0;
            }
            sample(&state);
        }
        panel.freeze_scroll = true;
        draw(&mut panel, &state); // warm: several collapsed entries per port
        clear();
    }
}
