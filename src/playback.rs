//! Record/playback dummy slots (task A2): capture both controller ports'
//! folded per-frame input into a named, per-family slot on disk, and replay
//! a slot deterministically onto one or both ports. This is three things at
//! once — a training feature (every surveyed trainer ships recording
//! slots), the frame lab's determinism instrument (`docs/frames.md` §3/§4
//! demand exact repeatability from the same save state), and a reproducible
//! bug-repro format.
//!
//! ## Why this state lives on `DebugState`, not a process-wide static
//! `debug/panels/input_log.rs` (task A1) uses its own `OnceLock<Mutex<..>>`
//! specifically because a concurrent edit at the time could not touch
//! `DebugState`'s shape. This feature has no such restriction, and its
//! control state genuinely belongs in `DebugState` (like `TrainingConfig`/
//! `RecordControl`): every test builds its own `DebugState::new()`, and
//! record/playback must be exercisable test-by-test without a shared global
//! silently mixing frames from unrelated tests running concurrently in other
//! threads (`cargo test`'s default). See `debug::RecordingSlot`/
//! `debug::PlaybackSlot`, which hold the actual fields.
//!
//! ## Determinism (docs/frames.md §3/§4 — this feature IS a frame-lab
//! instrument)
//! - [`tick`] is called once per REAL emulated frame, from `training::tick`,
//!   at the exact point that function already runs: inside
//!   `Frontend::run_frame`, AFTER `core.run()` and the bus-window refresh.
//!   Critically, `run_frame` reaches that point only when the core actually
//!   advanced (paused frames return earlier) — so [`tick`] never runs on a
//!   GUI-only fold. Nothing here can evaporate across a pause, which is
//!   exactly the documented `press_buttons`/injected-input failure mode this
//!   feature is built to avoid.
//! - Capture samples `ds.input_state`/`input_state2` — the POST-FOLD masks
//!   the game actually received this frame (the same source
//!   `debug/panels/input_log.rs` samples, for the same reason: it's what the
//!   game saw, not raw keyboard).
//! - Playback asserts frame `idx`'s mask via `DebugState::set_held_input`,
//!   which REPLACES a port's held set and never decays on its own — unlike
//!   the countdown arrays (`injected_input`/`injected_input2`), a held set
//!   survives any number of GUI-only folds unchanged, so a playback frame
//!   stays exactly what [`tick`] last set it to until [`tick`] runs again for
//!   the next real frame. Because [`tick`] runs exactly once per real frame,
//!   this is frame-exact by construction.
//!
//! **What IS guaranteed deterministic:** the `RoundStart` trigger. Playback
//! begins on the exact frame the fight gate transitions closed→open, which
//! is the same physical event on every replay from an identical PRE-round
//! save state.
//!
//! **What is NOT guaranteed deterministic:** the `Manual` trigger issued
//! against a LIVE (unpaused) session. `start_playback` arms `started=true`
//! immediately, and the very next real frame after that call plays frame 0 —
//! but which real frame that turns out to be depends on the real-world race
//! between when the MCP/panel call lands and when the emulation thread next
//! reaches [`tick`]. Two runs against the same save state can start the
//! sequence on different frames. To make `Manual` frame-exact, follow
//! docs/frames.md's own protocol: `pause` → issue `play_inputs` → `step` —
//! then the call and the very next `tick` are the same deterministic step,
//! with no wall-clock race. This is stated explicitly rather than shipped as
//! an implicit "usually works" claim.
//!
//! ## Precedence vs the training dummy (task A2 §4)
//! The training dummy (`training::tick`'s `DummyMode`) drives controller
//! port 1 (P2) through the COUNTDOWN array (`injected_input2`), refreshed
//! every real frame it runs. If a playback also targets port 1, both paths
//! would otherwise land in the same fold (`take_injected_input2` ORs the
//! countdown with the held set) and silently blend — exactly what task A2
//! forbids ("do not let both write silently; one must win, visibly"). The
//! rule: **playback always wins.** `training::tick_with` calls
//! [`active_on_port`] before writing any dummy bits into `injected_input2`
//! for a port playback is actively driving (`started && !done`), and skips
//! that write entirely — not a blend, a clean handoff. While a `RoundStart`
//! playback is merely ARMED (not yet triggered), the dummy is unaffected, so
//! the switch-over is visible exactly at the moment it happens. The Training
//! panel's "Driving" line names which of {dummy, playback} owns each port
//! every frame, so this is never a silent surprise.
//!
//! ## Save-state loads mid-playback
//! A `load_state` while a playback is in flight is a real, legitimate user
//! action — docs/frames.md's own frame lab does exactly this to reset for
//! the next trial. This module does NOT special-case it: the loaded state's
//! frame simply becomes whatever `idx` the playback happens to be at, and
//! playback keeps stepping its recorded mask sequence from there. A `Manual`
//! playback already in flight is unaffected by the load and just continues.
//! A `RoundStart` playback still ARMED re-evaluates the gate against the
//! freshly loaded state on the very next tick — exactly the "reset and
//! re-arm" behaviour the frame lab's repeat-trial workflow wants. If the
//! loaded state lands mid-round while `RoundStart` is armed, the trigger
//! correctly waits for the NEXT round (see `PlaybackTrigger::RoundStart`) —
//! a jump into the middle of a round never retroactively "already satisfies"
//! it.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::debug::{DebugState, PlaybackPort, PlaybackSlot, PlaybackTrigger, RecordingSlot};

/// On-disk slot schema version. A slot from a version this build doesn't
/// understand fails to load with a message naming the mismatch, never a
/// panic — see [`load_slot`].
pub const SLOT_VERSION: u32 = 1;

/// Hard cap on a single recording's length (~1 hour at 60 fps) — a forgotten
/// recording must not grow the process without bound. Hitting it auto-stops
/// and auto-saves whatever was captured so far (never silently drops it).
const MAX_RECORDING_FRAMES: usize = 216_000;

/// One named input slot on disk: both ports' per-frame masks plus enough
/// provenance to know what it was captured against (task A2 §1). Frame `i`
/// is `[p1_mask, p2_mask]` in `record::pack_mask`'s RETRO_DEVICE_ID bit
/// order — a recording always captures BOTH ports regardless of what a
/// later playback targets.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InputSlot {
    pub version: u32,
    pub family: String,
    pub port: String,
    pub created_at: u64,
    /// Best-effort provenance: `DebugState::state_note` at the moment
    /// recording started, or `None` if no state op had run yet this
    /// session. See `debug::RecordingSlot::state_note_at_start`.
    pub state_note_at_start: Option<String>,
    pub frames: Vec<[u16; 2]>,
}

/// Summary row for `list_input_slots` / the panel's slot list — everything
/// but the (potentially large) frame data.
#[derive(Clone, Debug, Serialize)]
pub struct SlotSummary {
    pub name: String,
    pub family: String,
    pub port: String,
    pub created_at: u64,
    pub frame_count: usize,
    pub state_note_at_start: Option<String>,
}

fn epoch_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Where a family's input slots live — `shadow/inputs/<family>/`, matching
/// the `shadow/<kind>/<family>/` convention `debug/panels/training.rs`
/// already uses for models/recordings/arenas. Relative to the launch cwd.
pub fn slots_dir(family: &str) -> PathBuf {
    PathBuf::from("shadow/inputs").join(family)
}

fn slot_file(family: &str, name: &str) -> PathBuf {
    slots_dir(family).join(format!("{name}.slot.json"))
}

/// Validate a user-supplied slot name: non-empty, no path separators or
/// `..` traversal — it becomes a filename directly.
fn sanitize_name(name: &str) -> Result<String, String> {
    let n = name.trim();
    if n.is_empty() {
        return Err("slot name must not be empty".into());
    }
    if n.contains('/') || n.contains('\\') || n == "." || n == ".." {
        return Err(format!("invalid slot name '{name}': no path separators or '..'"));
    }
    Ok(n.to_string())
}

/// Persist a slot to `shadow/inputs/<family>/<name>.slot.json`, creating the
/// directory if needed. Returns the written path.
pub fn save_slot(slot: &InputSlot, name: &str) -> Result<PathBuf, String> {
    let name = sanitize_name(name)?;
    let dir = slots_dir(&slot.family);
    std::fs::create_dir_all(&dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    let path = dir.join(format!("{name}.slot.json"));
    let json = serde_json::to_string_pretty(slot).map_err(|e| format!("serialize: {e}"))?;
    std::fs::write(&path, json).map_err(|e| format!("write {}: {e}", path.display()))?;
    Ok(path)
}

/// Load a named slot for `family`. Never panics: a missing, unreadable, or
/// corrupt file is an `Err` naming the problem — exactly like a corrupt
/// `TrainingConfig` sidecar (`TrainingConfig::merge_persisted`), a bad slot
/// file fails the ONE call that touched it, never the process.
pub fn load_slot(family: &str, name: &str) -> Result<InputSlot, String> {
    let name = sanitize_name(name)?;
    let path = slot_file(family, &name);
    let text =
        std::fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let slot: InputSlot = serde_json::from_str(&text)
        .map_err(|e| format!("{} is not a valid input slot: {e}", path.display()))?;
    if slot.version != SLOT_VERSION {
        return Err(format!(
            "{}: slot version {} unsupported (this build understands {SLOT_VERSION})",
            path.display(),
            slot.version
        ));
    }
    Ok(slot)
}

/// List every slot under `shadow/inputs/<family>/`, newest first. A slot
/// that fails to parse is SKIPPED (not fatal to the listing) — "absent means
/// absent" (docs/frames.md §2 rule 5) beats aborting the whole list over one
/// bad file.
pub fn list_slots(family: &str) -> Vec<SlotSummary> {
    let dir = slots_dir(family);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out: Vec<SlotSummary> = entries
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            let file_name = path.file_name()?.to_string_lossy().into_owned();
            let name = file_name.strip_suffix(".slot.json")?.to_string();
            let text = std::fs::read_to_string(&path).ok()?;
            let slot: InputSlot = serde_json::from_str(&text).ok()?;
            Some(SlotSummary {
                name,
                family: slot.family,
                port: slot.port,
                created_at: slot.created_at,
                frame_count: slot.frames.len(),
                state_note_at_start: slot.state_note_at_start,
            })
        })
        .collect();
    out.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    out
}

// ── recording control ───────────────────────────────────────────────────────

/// Begin capturing both ports into `name`. Fails if already recording (stop
/// first) or the name is invalid. `p` is the ACTIVE profile — an explicit
/// parameter (like `training::tick_with`'s), not `crate::profile::current()`
/// internally, so per-game tests that load their own local `GameProfile`
/// (the process's `current()` OnceLock is fixed to asurabld for the whole
/// test binary) exercise the SAME family/port this call actually records
/// against, instead of silently reading the global to a different game.
pub fn start_recording(ds: &mut DebugState, name: &str, p: &crate::profile::GameProfile) -> Result<(), String> {
    let name = sanitize_name(name)?;
    if ds.recording_slot.is_some() {
        return Err("already recording — stop it first".into());
    }
    ds.recording_slot = Some(RecordingSlot {
        name,
        family: p.family.family.clone(),
        port: p.port.port.clone(),
        state_note_at_start: ds.state_note.clone(),
        frames: Vec::new(),
    });
    Ok(())
}

/// Stop the active recording and save it to disk. Returns (path, frame
/// count). An empty (0-frame) recording still saves — an empty slot is
/// itself a legitimate (if useless) artifact, not an error.
pub fn stop_recording(ds: &mut DebugState) -> Result<(PathBuf, usize), String> {
    let rec = ds.recording_slot.take().ok_or("not recording")?;
    finish_recording(rec)
}

fn finish_recording(rec: RecordingSlot) -> Result<(PathBuf, usize), String> {
    let RecordingSlot { name, family, port, state_note_at_start, frames } = rec;
    let n = frames.len();
    let slot =
        InputSlot { version: SLOT_VERSION, family, port, created_at: epoch_secs(), state_note_at_start, frames };
    let path = save_slot(&slot, &name)?;
    Ok((path, n))
}

// ── playback control ────────────────────────────────────────────────────────

/// Load `name` from `p`'s family and arm it for playback on `port` with
/// `trigger`. `p` is an explicit parameter for the same reason
/// [`start_recording`]'s is — see its doc. Refuses if another playback is
/// already active/armed, or the slot's OWN recorded family metadata doesn't
/// match `p`'s (a slot file copied/misplaced into the wrong family's
/// directory) — cross-family playback is almost certainly a mistake (the
/// per-family data-root convention `training.rs`'s panel already enforces
/// for models/recordings/arenas). Returns the frame count.
pub fn start_playback(
    ds: &mut DebugState,
    name: &str,
    port: PlaybackPort,
    trigger: PlaybackTrigger,
    p: &crate::profile::GameProfile,
) -> Result<usize, String> {
    if ds.playback_slot.is_some() {
        return Err("a playback is already active — stop it first".into());
    }
    let family = p.family.family.clone();
    let slot = load_slot(&family, name)?;
    if slot.family != family {
        return Err(format!(
            "slot '{name}' was recorded for family '{}', not the loaded '{family}'",
            slot.family
        ));
    }
    let n = slot.frames.len();
    ds.playback_slot = Some(PlaybackSlot {
        name: name.to_string(),
        port,
        trigger,
        frames: slot.frames,
        started: false,
        idx: 0,
        done: false,
        gate_baseline: None,
    });
    Ok(n)
}

/// Stop an active/armed playback immediately, releasing any ports it held.
pub fn stop_playback(ds: &mut DebugState) -> Result<(), String> {
    let pb = ds.playback_slot.take().ok_or("no playback active")?;
    release_ports(ds, pb.port);
    Ok(())
}

fn release_ports(ds: &mut DebugState, port: PlaybackPort) {
    if port.drives(0) {
        ds.clear_held_input(0, None);
    }
    if port.drives(1) {
        ds.clear_held_input(1, None);
    }
}

/// True while an ACTIVE (triggered, not yet finished) playback is asserting
/// bits onto `port` (0/1) this frame. `training::tick_with` uses this to
/// suppress the training dummy's write for that port instead of blending
/// with it — see this module's precedence doc.
pub fn active_on_port(ds: &DebugState, port: usize) -> bool {
    ds.playback_slot.as_ref().is_some_and(|pb| pb.started && !pb.done && pb.port.drives(port))
}

fn unpack_mask(mask: u16) -> [bool; 12] {
    let mut out = [false; 12];
    for (i, b) in out.iter_mut().enumerate() {
        *b = mask & (1 << i) != 0;
    }
    out
}

/// Advance recording + playback by one REAL emulated frame. Called from
/// `training::tick_with`, unconditionally — independent of
/// `TrainingConfig::enabled`, since capture/replay is its own feature, not a
/// training-mode enforcement. `p` is `tick_with`'s own profile parameter,
/// threaded through rather than read via `crate::profile::current()` here —
/// see [`start_recording`]'s doc for why (per-game tests load their own
/// local `GameProfile` distinct from the process-wide `current()`, which
/// stays fixed to asurabld for the whole test binary).
pub fn tick(ds: &mut DebugState, _frame: u64, p: &crate::profile::GameProfile) {
    // Release a playback that finished on a PREVIOUS call to this function,
    // deferred by exactly one tick so the caller can observe the FINAL
    // frame's asserted bits (this same call's "apply" step, or an earlier
    // one) before they're cleared. Releasing within the SAME tick that sets
    // `done` would clear the bits before anything downstream (the input
    // fold) ever reads them — the playback's last frame would never
    // actually take effect.
    let previously_finished = ds.playback_slot.as_ref().is_some_and(|pb| pb.done);
    if previously_finished {
        if let Some(pb) = ds.playback_slot.take() {
            release_ports(ds, pb.port);
            let note = format!("playback '{}' finished ({} frames)", pb.name, pb.frames.len());
            ds.log(format!("📼 {note}"));
            ds.playback_note = Some(note);
        }
    }

    // ── capture ──────────────────────────────────────────────────────────
    if ds.recording_slot.is_some() {
        let p0 = crate::record::pack_mask(&ds.input_state);
        let p1 = crate::record::pack_mask(&ds.input_state2);
        if let Some(rec) = ds.recording_slot.as_mut() {
            rec.frames.push([p0, p1]);
        }
        let hit_cap =
            ds.recording_slot.as_ref().is_some_and(|r| r.frames.len() >= MAX_RECORDING_FRAMES);
        if hit_cap {
            if let Some(rec) = ds.recording_slot.take() {
                let name = rec.name.clone();
                match finish_recording(rec) {
                    Ok((path, n)) => {
                        let note =
                            format!("recording '{name}' hit the {MAX_RECORDING_FRAMES}-frame cap — auto-stopped, {n} frames saved to {}", path.display());
                        ds.log(format!("📼 {note}"));
                        ds.recording_note = Some(note);
                    }
                    Err(e) => {
                        let note = format!("recording '{name}' hit the frame cap but FAILED to save: {e}");
                        ds.log(format!("📼 {note}"));
                        ds.recording_note = Some(note);
                    }
                }
            }
        }
    }

    // ── playback: trigger check ─────────────────────────────────────────
    let needs_gate = ds
        .playback_slot
        .as_ref()
        .is_some_and(|pb| !pb.started && pb.trigger == PlaybackTrigger::RoundStart);
    let gate_open = needs_gate.then(|| crate::gate::eval_gate(ds, p));
    if let Some(pb) = ds.playback_slot.as_mut() {
        if !pb.started {
            pb.started = match pb.trigger {
                PlaybackTrigger::Manual => true,
                PlaybackTrigger::RoundStart => {
                    let open = gate_open.unwrap_or(false);
                    let rising = match pb.gate_baseline {
                        Some(prev) => open && !prev,
                        None => false, // first observation only seeds the baseline
                    };
                    pb.gate_baseline = Some(open);
                    rising
                }
            };
        }
    }

    // ── playback: apply this frame's mask ───────────────────────────────
    let apply: Option<(PlaybackPort, [u16; 2])> = ds.playback_slot.as_mut().and_then(|pb| {
        if pb.started && !pb.done && pb.idx < pb.frames.len() {
            let masks = pb.frames[pb.idx];
            pb.idx += 1;
            if pb.idx >= pb.frames.len() {
                pb.done = true;
            }
            Some((pb.port, masks))
        } else {
            None
        }
    });
    if let Some((port, masks)) = apply {
        if port.drives(0) {
            ds.set_held_input(0, unpack_mask(masks[0]));
        }
        if port.drives(1) {
            ds.set_held_input(1, unpack_mask(masks[1]));
        }
    }
    // NOTE: `done` may have just been set true above (the last frame was
    // just applied) — release happens on the NEXT call to `tick`, at the
    // top of this function. See that comment for why.
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::debug::BusWindowCfg;

    fn set_bits(bits: &mut [bool; 12], idxs: &[usize]) {
        for &i in idxs {
            bits[i] = true;
        }
    }

    // ── name/path hygiene ────────────────────────────────────────────────

    #[test]
    fn sanitize_name_rejects_traversal_and_empty() {
        assert!(sanitize_name("goat-vs-rosemary").is_ok());
        assert!(sanitize_name("  padded  ").is_ok());
        assert_eq!(sanitize_name("  padded  ").unwrap(), "padded");
        assert!(sanitize_name("").is_err());
        assert!(sanitize_name("   ").is_err());
        assert!(sanitize_name("a/b").is_err());
        assert!(sanitize_name("a\\b").is_err());
        assert!(sanitize_name("..").is_err());
        assert!(sanitize_name(".").is_err());
    }

    #[test]
    fn slots_dir_is_per_family() {
        assert_eq!(slots_dir("asurabld"), PathBuf::from("shadow/inputs/asurabld"));
        assert_eq!(slots_dir("mk2"), PathBuf::from("shadow/inputs/mk2"));
    }

    // ── round-trip through disk ──────────────────────────────────────────

    #[test]
    fn slot_round_trips_through_disk() {
        let dir = std::env::temp_dir().join(format!("rustretro_playback_test_{}", std::process::id()));
        let family = format!("test-family-{}", std::process::id());
        let _ = std::fs::remove_dir_all(dir.join("shadow/inputs").join(&family));

        // Redirect via a per-test cwd would be invasive; instead exercise
        // save_slot/load_slot directly against a real relative dir under the
        // repo (cleaned up after) — matches how other sidecar tests in this
        // codebase (e.g. TrainingConfig) use temp_dir for the FILE but this
        // module's paths are intentionally cwd-relative (§5 of the task:
        // consistent with shadow/recordings, shadow/arenas, shadow/models).
        let slot = InputSlot {
            version: SLOT_VERSION,
            family: family.clone(),
            port: "arcade".into(),
            created_at: 1_700_000_000,
            state_note_at_start: Some("loaded shadow/arenas/x/current.state (123 bytes) @ frame 9".into()),
            frames: vec![[0, 0], [1 << 7, 1 << 8], [0, 0]],
        };
        let path = save_slot(&slot, "roundtrip").expect("save");
        assert!(path.ends_with("roundtrip.slot.json"));

        let loaded = load_slot(&family, "roundtrip").expect("load");
        assert_eq!(loaded, slot);

        // list_slots sees it.
        let list = list_slots(&family);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "roundtrip");
        assert_eq!(list[0].frame_count, 3);
        assert_eq!(list[0].state_note_at_start, slot.state_note_at_start);

        let _ = std::fs::remove_dir_all(PathBuf::from("shadow/inputs").join(&family));
    }

    #[test]
    fn corrupt_slot_file_fails_gracefully_without_panicking() {
        let family = format!("test-corrupt-{}", std::process::id());
        let dir = slots_dir(&family);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bad.slot.json");
        std::fs::write(&path, b"{ this is not valid json at all").unwrap();

        let result = load_slot(&family, "bad");
        assert!(result.is_err(), "corrupt file must be an Err, not a panic");

        // list_slots must also survive it (skips the bad entry) rather than
        // propagating the parse failure.
        std::fs::write(dir.join("good.slot.json"), serde_json::to_string(&InputSlot {
            version: SLOT_VERSION,
            family: family.clone(),
            port: "arcade".into(),
            created_at: 1,
            state_note_at_start: None,
            frames: vec![],
        }).unwrap()).unwrap();
        let list = list_slots(&family);
        assert_eq!(list.len(), 1, "corrupt slot skipped, good slot still listed: {list:?}");
        assert_eq!(list[0].name, "good");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_slot_is_an_error_not_a_panic() {
        let family = format!("test-missing-{}", std::process::id());
        let result = load_slot(&family, "nope");
        assert!(result.is_err());
    }

    #[test]
    fn unsupported_version_is_rejected() {
        let family = format!("test-version-{}", std::process::id());
        let dir = slots_dir(&family);
        std::fs::create_dir_all(&dir).unwrap();
        let mut slot = InputSlot {
            version: 999,
            family: family.clone(),
            port: "arcade".into(),
            created_at: 1,
            state_note_at_start: None,
            frames: vec![],
        };
        std::fs::write(dir.join("future.slot.json"), serde_json::to_string(&slot).unwrap()).unwrap();
        let err = load_slot(&family, "future").unwrap_err();
        assert!(err.contains("version"), "{err}");

        // Sanity: version SLOT_VERSION round-trips fine (proves the check is
        // actually gated on the version field, not always failing).
        slot.version = SLOT_VERSION;
        std::fs::write(dir.join("ok.slot.json"), serde_json::to_string(&slot).unwrap()).unwrap();
        assert!(load_slot(&family, "ok").is_ok());

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── tick: capture ────────────────────────────────────────────────────

    #[test]
    fn recording_captures_both_ports_post_fold_frame_for_frame() {
        let p = crate::profile::init_for_tests();
        let mut ds = DebugState::new();
        start_recording(&mut ds, "cap-test", p).unwrap();

        // Frame 1: P1 holds Right (bit 7), P2 idle.
        set_bits(&mut ds.input_state, &[7]);
        tick(&mut ds, 1, p);
        // Frame 2: P1 idle, P2 holds A (bit 8).
        ds.input_state = [false; 12];
        set_bits(&mut ds.input_state2, &[8]);
        tick(&mut ds, 2, p);

        let (path, n) = stop_recording(&mut ds).unwrap();
        assert_eq!(n, 2);
        let slot = load_slot(&p.family.family, "cap-test").unwrap();
        assert_eq!(slot.frames, vec![[1 << 7, 0], [0, 1 << 8]]);
        assert_eq!(slot.state_note_at_start, None);

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn stop_recording_without_starting_is_an_error() {
        let mut ds = DebugState::new();
        assert!(stop_recording(&mut ds).is_err());
    }

    #[test]
    fn cannot_start_a_second_recording_while_one_is_active() {
        let p = crate::profile::init_for_tests();
        let mut ds = DebugState::new();
        start_recording(&mut ds, "one", p).unwrap();
        assert!(start_recording(&mut ds, "two", p).is_err());
        let _ = stop_recording(&mut ds);
        let _ = std::fs::remove_file(slot_file(&p.family.family, "one"));
    }

    // ── tick: playback ───────────────────────────────────────────────────

    /// Record a short sequence, then play it back and confirm the EXACT
    /// recorded mask sequence comes out frame-for-frame via
    /// `set_held_input`/`take_injected_input` — the acceptance bar.
    #[test]
    fn playback_reproduces_the_exact_recorded_mask_sequence() {
        let p = crate::profile::init_for_tests();
        let family = p.family.family.clone();
        let frames = vec![[1u16 << 7, 0u16], [0, 1 << 8], [1 << 6 | 1 << 8, 0]];
        let slot = InputSlot {
            version: SLOT_VERSION,
            family: family.clone(),
            port: p.port.port.clone(),
            created_at: 1,
            state_note_at_start: None,
            frames: frames.clone(),
        };
        let path = save_slot(&slot, "play-test").unwrap();

        let mut ds = DebugState::new();
        let n = start_playback(&mut ds, "play-test", PlaybackPort::Both, PlaybackTrigger::Manual, p)
            .unwrap();
        assert_eq!(n, 3);

        let mut observed = Vec::new();
        for f in 1..=3u64 {
            tick(&mut ds, f, p);
            observed.push((
                crate::record::pack_mask(&ds.take_injected_input()),
                crate::record::pack_mask(&ds.take_injected_input2()),
            ));
        }
        let expected: Vec<(u16, u16)> = frames.iter().map(|m| (m[0], m[1])).collect();
        assert_eq!(observed, expected);

        // One more tick: the LAST frame's release is deferred by one tick
        // (see `tick`'s doc) — this is what actually clears the ports.
        tick(&mut ds, 4, p);
        assert_eq!(ds.take_injected_input(), [false; 12]);
        assert_eq!(ds.take_injected_input2(), [false; 12]);
        assert!(ds.playback_slot.is_none());

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn playback_can_target_a_single_port_only() {
        let p = crate::profile::init_for_tests();
        let family = p.family.family.clone();
        let slot = InputSlot {
            version: SLOT_VERSION,
            family: family.clone(),
            port: "arcade".into(),
            created_at: 1,
            state_note_at_start: None,
            frames: vec![[1 << 7, 1 << 8]],
        };
        let path = save_slot(&slot, "p1-only").unwrap();

        let mut ds = DebugState::new();
        start_playback(&mut ds, "p1-only", PlaybackPort::P1, PlaybackTrigger::Manual, p).unwrap();
        tick(&mut ds, 1, p);
        assert!(ds.take_injected_input()[7], "P1 driven");
        assert_eq!(ds.take_injected_input2(), [false; 12], "P2 left untouched");

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn round_start_trigger_waits_for_the_gate_rising_edge() {
        let p = crate::profile::init_for_tests();
        let family = p.family.family.clone();
        let slot = InputSlot {
            version: SLOT_VERSION,
            family: family.clone(),
            port: p.port.port.clone(),
            created_at: 1,
            state_note_at_start: None,
            frames: vec![[1 << 7, 0]],
        };
        let path = save_slot(&slot, "round-start-test").unwrap();

        let mut ds = DebugState::new();
        assert!(ds.install_bus_window(BusWindowCfg {
            name: "wram-playback-test".into(),
            addr: 0x400000,
            len: 0x10000,
            interval: 1,
            flags: crate::libretro::RETRO_MEMDESC_SYSTEM_RAM,
        }));
        start_playback(&mut ds, "round-start-test", PlaybackPort::P1, PlaybackTrigger::RoundStart, p)
            .unwrap();

        // Gate closed (fresh bus window reads all zero — asurabld's gate
        // needs health in range, which zero is not): armed, not asserting.
        tick(&mut ds, 1, p);
        assert_eq!(ds.take_injected_input(), [false; 12], "still armed — gate closed");
        assert!(!ds.playback_slot.as_ref().unwrap().started);

        // Open the gate (health in range for both blocks + timer nonzero,
        // matching asurabld_scene()'s recipe in training.rs's tests).
        let hoff = p.field_off("health").unwrap().0;
        assert!(ds.write_addr((p.block1() + hoff) as usize, 1, 100));
        assert!(ds.write_addr((p.block2() + hoff) as usize, 1, 100));
        assert!(ds.write_addr(p.global("round_timer").unwrap() as usize, 1, 0x99));
        assert!(crate::gate::eval_gate(&ds, p), "gate must actually be open now");

        tick(&mut ds, 2, p);
        assert!(ds.take_injected_input()[7], "rising edge triggered playback");

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn start_playback_refuses_a_second_concurrent_playback() {
        let p = crate::profile::init_for_tests();
        let family = p.family.family.clone();
        let slot = InputSlot {
            version: SLOT_VERSION,
            family: family.clone(),
            port: "arcade".into(),
            created_at: 1,
            state_note_at_start: None,
            frames: vec![[0, 0]],
        };
        let path = save_slot(&slot, "concurrent").unwrap();

        let mut ds = DebugState::new();
        start_playback(&mut ds, "concurrent", PlaybackPort::Both, PlaybackTrigger::Manual, p).unwrap();
        assert!(
            start_playback(&mut ds, "concurrent", PlaybackPort::Both, PlaybackTrigger::Manual, p)
                .is_err()
        );
        let _ = stop_playback(&mut ds);
        let _ = std::fs::remove_file(path);
    }

    /// A slot file whose OWN recorded `family` metadata doesn't match the
    /// directory it was found in (e.g. hand-copied from another family's
    /// `shadow/inputs/` tree) must be refused, not silently played back
    /// against the wrong game's button layout.
    #[test]
    fn start_playback_refuses_a_cross_family_slot() {
        let p = crate::profile::init_for_tests();
        // Deliberately mismatched: written into `p`'s OWN directory (so
        // `load_slot` finds it) but with foreign `family` metadata inside.
        let slot = InputSlot {
            version: SLOT_VERSION,
            family: "some-other-family".into(),
            port: "arcade".into(),
            created_at: 1,
            state_note_at_start: None,
            frames: vec![[0, 0]],
        };
        let dir = slots_dir(&p.family.family);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("foreign.slot.json");
        std::fs::write(&path, serde_json::to_string(&slot).unwrap()).unwrap();

        let mut ds = DebugState::new();
        let err = start_playback(&mut ds, "foreign", PlaybackPort::Both, PlaybackTrigger::Manual, p)
            .unwrap_err();
        assert!(err.contains("some-other-family"), "{err}");

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn stop_playback_without_one_active_is_an_error() {
        let mut ds = DebugState::new();
        assert!(stop_playback(&mut ds).is_err());
    }

    #[test]
    fn active_on_port_is_false_while_merely_armed() {
        let p = crate::profile::init_for_tests();
        let family = p.family.family.clone();
        let slot = InputSlot {
            version: SLOT_VERSION,
            family: family.clone(),
            port: "arcade".into(),
            created_at: 1,
            state_note_at_start: None,
            frames: vec![[0, 0]],
        };
        let path = save_slot(&slot, "armed-check").unwrap();

        let mut ds = DebugState::new();
        assert!(ds.install_bus_window(BusWindowCfg {
            name: "wram-armed-test".into(),
            addr: 0x400000,
            len: 0x10000,
            interval: 1,
            flags: crate::libretro::RETRO_MEMDESC_SYSTEM_RAM,
        }));
        start_playback(&mut ds, "armed-check", PlaybackPort::P2, PlaybackTrigger::RoundStart, p)
            .unwrap();
        assert!(!active_on_port(&ds, 1), "armed but not yet triggered must not claim the port");
        tick(&mut ds, 1, p); // gate closed on a blank bus window — stays armed
        assert!(!active_on_port(&ds, 1));

        let _ = std::fs::remove_file(path);
    }
}
