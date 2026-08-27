//! Native in-app shadow runner: fight your own recorded ghost with a keypress.
//!
//! Loads a fitted kNN case-retrieval model (`shadow/models/<name>/`: the
//! `cases.npz` + `meta.json` written by `shadow_train.knn.KnnPolicy.save`)
//! and drives controller port 1 (P2) from inside the emulator process — no
//! second process, no MCP round-trips. This is a Rust mirror of the deployed
//! Python stack, and it must stay behaviorally equivalent to it:
//!
//! - policy math  = `shadow/train/shadow_train/knn.py` (soft retrieval:
//!   sigma from the k-th neighbor, `exp(-(d/sigma)^2)` weights over the
//!   `WIDE_K` nearest, per-head weighted-count softmax with temperature),
//! - feature math = `shadow/train/shadow_train/runtime.py` / `dataset.py`
//!   (21 scalars, K=4 stacked decision ticks, opponent read one decision
//!   tick stale, holds from the bot's own emitted mask, hitstun =
//!   combo-counter changed recently),
//! - deploy loop  = `shadow/play.py` (gate, per-round larger-X auto-anchor,
//!   buffer reset on the gate's rising edge).
//!
//! Golden-value unit tests below pin the npz reader and the vote math to
//! numbers extracted from the Python implementation on the real
//! `shadow/models/asurabld/goat-v2` model.
//!
//! ## Wiring (design decision)
//! The runner is owned by `Frontend` (`shadow: Option<ShadowRunner>`), not
//! `DebugState`: it needs no cross-thread access — `Frontend::run_frame`
//! calls [`ShadowRunner::tick`] on the emu thread right after
//! `training::tick`, under the same brief `DebugState` lock pattern. Reads go
//! through `DebugState::read_addr` (the per-frame bus-window snapshot);
//! injection goes through `DebugState::injected_input2` with 2-frame hold
//! counts refreshed every frame (the `training.rs` dummy idiom — counts
//! outlive one GUI input fold, which can run at display rate, without
//! latching), while the *decision* that picks the held mask runs every
//! `P` = 8 emulated frames (~7.5 Hz), mirroring `play.py`'s
//! `press_buttons(frames=8)` cadence. Because the shadow runs after
//! `training::tick`, its injection overrides a non-Free training dummy —
//! leave the dummy on Free when fighting the shadow.

use crate::debug::DebugState;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::path::Path;

// ── contract constants (mirror shadow_train.dataset) ────────────────────────

/// Scalar feature names in vector order — the runtime's expectation. A model
/// whose `meta.json` `feature_names` differ is rejected at load with both
/// lists in the error.
pub const SCALAR_FEATURES: [&str; 21] = [
    "dist_x", "dy", "me_airborne", "me_height", "me_fwd_hold", "me_back_hold",
    "me_anim", "me_timer", "opp_airborne", "opp_height", "opp_anim",
    "opp_timer", "facing_sign", "me_health", "opp_health", "health_lead",
    "me_meter", "opp_meter", "me_hitstun", "opp_hitstun", "me_corner",
];

/// Soft-retrieval electorate size (knn.py `WIDE_K`).
const WIDE_K: usize = 100;

// RETRO joypad bit indices for MOVEMENT (dataset.py / src/mcp/server.rs
// order). Only the directions are engine knowledge — attack buttons come
// from the profile's `attack_chords` table, compiled at model load.
const BIT_UP: u16 = 4;
const BIT_DOWN: u16 = 5;
const BIT_LEFT: u16 = 6;
const BIT_RIGHT: u16 = 7;

// ── profile-resolved addresses (resolved ONCE at load; no per-frame maps) ───
//
// Two tiers, per the model-meta-driven requirement contract
// (RECORDER_V3.md §4.2 / docs/game-profiles.md rule 3):
//
// - [`RunnerAddrs`]: resolved ONCE per `ShadowRunner::load`, independent of
//   any single model. Only `x` is unconditionally required — every fitted
//   model needs it (dataset.py's `_REQUIRED_FIELDS`/`scalar_features_for`:
//   `dist_x` never drops, and the runner's own round-anchor probe needs it
//   before any model is even chosen). `char_id` is best-effort (matchup-set
//   switching degrades to "never switches" without it, not an error).
// - [`FeatureAddrs`]: resolved PER LOADED MODEL from that model's own
//   `feature_names`, so a model without hitstun features doesn't require
//   `hitstun_sources`, one without `me_corner`'s calibration doesn't require
//   anything extra (corner only needs `x`, already covered), etc. Errors name
//   both the feature that wants the data and the missing profile piece.

/// Addresses/flags every model shares, resolved once at load.
#[derive(Clone, Copy)]
struct RunnerAddrs {
    block1: u32,
    block2: u32,
    little: bool,
    /// Absolute per-block `x` addresses — universally required (see module
    /// comment). Resolved via `field_addr`, so both the block-offset and the
    /// global-sourced variants (MK2 arcade's p1_x/p2_x) work.
    x1: u32,
    x2: u32,
    /// `char_id` fighter-field offset, if the profile maps one (matchup-set
    /// switching; absent = the runner never switches models mid-set).
    char_id_off: Option<u32>,
}

impl RunnerAddrs {
    fn from_profile(p: &crate::profile::GameProfile) -> Result<RunnerAddrs, String> {
        let err = || {
            "profile: shadow runner needs fighter field 'x' (dist_x + the round-start \
             anchor probe — required by every fitted model)"
                .to_string()
        };
        let (x1, _size) = p.field_addr(1, "x").ok_or_else(err)?;
        let (x2, _size) = p.field_addr(2, "x").ok_or_else(err)?;
        Ok(RunnerAddrs {
            block1: p.block1(),
            block2: p.block2(),
            little: p.port.memory.endianness == "little",
            x1,
            x2,
            char_id_off: p.field_off("char_id").map(|(off, _)| off),
        })
    }
}

/// Fighter-block field offsets a SPECIFIC model's feature list needs.
/// `None` = this profile doesn't map it (and no loaded model needs it, else
/// [`FeatureAddrs::from_profile`] would have already failed to load).
#[derive(Clone, Copy, Default)]
struct FighterOffs {
    y: Option<u32>,
    anim: Option<u32>,
    timer: Option<u32>,
    /// Opportunistic, not gated by a specific feature: §4.2's fallback rule
    /// (`facing_sign = sign(opp.x - me.x)`) means `facing_sign` never fails
    /// to resolve — it just changes HOW it's computed when the profile has
    /// no `facing` field.
    facing: Option<u32>,
    health: Option<u32>,
    meter: Option<u32>,
    meter_max: Option<u32>,
}

/// Per-model resolved addresses: fighter-field offsets this model's feature
/// list needs, plus the hitstun global pair if it needs hitstun features.
#[derive(Clone, Copy)]
struct FeatureAddrs {
    offs: FighterOffs,
    /// (block1's hitstun-evidence global, block2's), only when a model
    /// declares `me_hitstun`/`opp_hitstun` among its `feature_names`.
    hitstun_globals: Option<(u32, u32)>,
}

impl FeatureAddrs {
    /// Resolve exactly what `feature_names` needs from `p` — per RECORDER_V3
    /// §4.2's availability table. Errors name both the feature and the
    /// missing profile piece.
    fn from_profile(
        p: &crate::profile::GameProfile,
        feature_names: &[String],
    ) -> Result<FeatureAddrs, String> {
        let want = |f: &str| feature_names.iter().any(|n| n == f);
        let need_field = |name: &str, feats: &str| -> Result<u32, String> {
            p.field_off(name).map(|(off, _)| off).ok_or_else(|| {
                format!(
                    "profile: feature '{feats}' needs fighter field '{name}' — this \
                     profile doesn't map it"
                )
            })
        };

        let mut offs = FighterOffs::default();
        if want("dy") || want("me_airborne") || want("me_height") || want("opp_airborne") || want("opp_height") {
            let feats: Vec<&str> = ["dy", "me_airborne", "me_height", "opp_airborne", "opp_height"]
                .into_iter()
                .filter(|f| want(f))
                .collect();
            offs.y = Some(need_field("y", &feats.join("/"))?);
        }
        if want("me_anim") || want("opp_anim") {
            offs.anim = Some(need_field("anim", "me_anim/opp_anim")?);
        }
        if want("me_timer") || want("opp_timer") {
            offs.timer = Some(need_field("timer", "me_timer/opp_timer")?);
        }
        if want("me_health") || want("opp_health") || want("health_lead") {
            offs.health = Some(need_field("health", "me_health/opp_health/health_lead")?);
        }
        if want("me_meter") || want("opp_meter") {
            offs.meter = Some(need_field("meter", "me_meter/opp_meter")?);
            offs.meter_max = Some(need_field("meter_max", "me_meter/opp_meter")?);
        }
        offs.facing = p.field_off("facing").map(|(off, _)| off);

        let hitstun_globals = if want("me_hitstun") || want("opp_hitstun") {
            let hs = p.port.hitstun_sources.as_ref().ok_or_else(|| {
                "profile: feature 'me_hitstun'/'opp_hitstun' needs 'hitstun_sources' — \
                 this profile declares none"
                    .to_string()
            })?;
            let g1name = hs.get("block1").ok_or_else(|| {
                "profile: feature 'me_hitstun'/'opp_hitstun' needs hitstun_sources.block1 \
                 — this profile's hitstun_sources has no 'block1' entry"
                    .to_string()
            })?;
            let g2name = hs.get("block2").ok_or_else(|| {
                "profile: feature 'me_hitstun'/'opp_hitstun' needs hitstun_sources.block2 \
                 — this profile's hitstun_sources has no 'block2' entry"
                    .to_string()
            })?;
            let g1 = p.global(g1name).ok_or_else(|| {
                format!(
                    "profile: feature 'me_hitstun'/'opp_hitstun' needs global '{g1name}' \
                     (hitstun_sources.block1) — not mapped"
                )
            })?;
            let g2 = p.global(g2name).ok_or_else(|| {
                format!(
                    "profile: feature 'me_hitstun'/'opp_hitstun' needs global '{g2name}' \
                     (hitstun_sources.block2) — not mapped"
                )
            })?;
            Some((g1, g2))
        } else {
            None
        };

        Ok(FeatureAddrs { offs, hitstun_globals })
    }
}

// ── minimal .npz reader (uncompressed zip of .npy, np.savez output) ─────────

/// One decoded `.npy` array. Only the dtypes `np.savez` emits for this model
/// family are supported: little-endian f4/f8/i8, C order.
pub struct Npy {
    pub shape: Vec<usize>,
    pub data: NpyData,
}

pub enum NpyData {
    F32(Vec<f32>),
    F64(Vec<f64>),
    I64(Vec<i64>),
}

impl Npy {
    fn elem_count(&self) -> usize {
        self.shape.iter().product::<usize>().max(1) // () scalar → 1
    }
    pub fn as_f64(&self) -> Vec<f64> {
        match &self.data {
            NpyData::F32(v) => v.iter().map(|&x| x as f64).collect(),
            NpyData::F64(v) => v.clone(),
            NpyData::I64(v) => v.iter().map(|&x| x as f64).collect(),
        }
    }
    pub fn as_i64(&self) -> Vec<i64> {
        match &self.data {
            NpyData::F32(v) => v.iter().map(|&x| x as i64).collect(),
            NpyData::F64(v) => v.iter().map(|&x| x as i64).collect(),
            NpyData::I64(v) => v.clone(),
        }
    }
}

fn le_u16(b: &[u8], off: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(off..off + 2)?.try_into().ok()?))
}
fn le_u32(b: &[u8], off: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(off..off + 4)?.try_into().ok()?))
}

/// Parse a numpy `.npz` (a zip archive) containing only STORED (uncompressed)
/// entries — which is exactly what `np.savez` writes. Walks the zip central
/// directory (robust against streamed local headers whose sizes are deferred
/// to data descriptors) and decodes each member with [`parse_npy`]. Entry
/// names are returned without their `.npy` suffix, matching `np.load` keys.
pub fn parse_npz(bytes: &[u8]) -> Result<HashMap<String, Npy>, String> {
    // End-of-central-directory record: scan back for its signature.
    const EOCD_SIG: u32 = 0x0605_4B50;
    const CEN_SIG: u32 = 0x0201_4B50;
    const LOC_SIG: u32 = 0x0403_4B50;
    if bytes.len() < 22 {
        return Err("npz: file too small to be a zip".into());
    }
    let eocd = (0..=bytes.len() - 22)
        .rev()
        .find(|&i| le_u32(bytes, i) == Some(EOCD_SIG))
        .ok_or("npz: no zip end-of-central-directory record")?;
    let n_entries = le_u16(bytes, eocd + 10).ok_or("npz: truncated EOCD")? as usize;
    let mut cd = le_u32(bytes, eocd + 16).ok_or("npz: truncated EOCD")? as usize;

    let mut out = HashMap::new();
    for _ in 0..n_entries {
        if le_u32(bytes, cd) != Some(CEN_SIG) {
            return Err(format!("npz: bad central-directory entry at 0x{cd:X}"));
        }
        let method = le_u16(bytes, cd + 10).ok_or("npz: truncated entry")?;
        let csize = le_u32(bytes, cd + 20).ok_or("npz: truncated entry")? as usize;
        let usize_ = le_u32(bytes, cd + 24).ok_or("npz: truncated entry")? as usize;
        let nlen = le_u16(bytes, cd + 28).ok_or("npz: truncated entry")? as usize;
        let xlen = le_u16(bytes, cd + 30).ok_or("npz: truncated entry")? as usize;
        let clen = le_u16(bytes, cd + 32).ok_or("npz: truncated entry")? as usize;
        let lho = le_u32(bytes, cd + 42).ok_or("npz: truncated entry")? as usize;
        let name = String::from_utf8_lossy(
            bytes.get(cd + 46..cd + 46 + nlen).ok_or("npz: truncated name")?,
        )
        .into_owned();
        cd += 46 + nlen + xlen + clen;

        if method != 0 {
            return Err(format!(
                "npz: entry '{name}' uses compression method {method}; only STORED \
                 (np.savez, not savez_compressed) is supported"
            ));
        }
        if csize == 0xFFFF_FFFF || usize_ == 0xFFFF_FFFF {
            return Err(format!("npz: entry '{name}' needs zip64 (unsupported)"));
        }
        // Data offset comes from the LOCAL header's own name/extra lengths
        // (they may differ from the central copy).
        if le_u32(bytes, lho) != Some(LOC_SIG) {
            return Err(format!("npz: entry '{name}': bad local header at 0x{lho:X}"));
        }
        let lnlen = le_u16(bytes, lho + 26).ok_or("npz: truncated local header")? as usize;
        let lxlen = le_u16(bytes, lho + 28).ok_or("npz: truncated local header")? as usize;
        let start = lho + 30 + lnlen + lxlen;
        let data = bytes
            .get(start..start + csize)
            .ok_or_else(|| format!("npz: entry '{name}' data out of bounds"))?;
        let key = name.strip_suffix(".npy").unwrap_or(&name).to_string();
        let arr = parse_npy(data).map_err(|e| format!("npz entry '{name}': {e}"))?;
        out.insert(key, arr);
    }
    Ok(out)
}

/// Decode a `.npy` v1/v2 buffer: magic, header length, then a Python-dict
/// literal `{'descr': '<f4', 'fortran_order': False, 'shape': (13405, 84), }`.
pub fn parse_npy(bytes: &[u8]) -> Result<Npy, String> {
    if bytes.len() < 10 || &bytes[..6] != b"\x93NUMPY" {
        return Err("bad npy magic".into());
    }
    let (hlen, hstart) = match bytes[6] {
        1 => (le_u16(bytes, 8).ok_or("truncated npy header")? as usize, 10),
        2 | 3 => (le_u32(bytes, 8).ok_or("truncated npy header")? as usize, 12),
        v => return Err(format!("unsupported npy version {v}")),
    };
    let header = std::str::from_utf8(
        bytes.get(hstart..hstart + hlen).ok_or("truncated npy header")?,
    )
    .map_err(|_| "npy header not utf-8")?;

    // Tiny field extractors over the dict literal — full Python parsing is not
    // needed for the fixed key set numpy writes.
    let quoted = |key: &str| -> Option<String> {
        let at = header.find(&format!("'{key}'"))?;
        let rest = &header[at + key.len() + 2..];
        let colon = rest.find(':')?;
        let rest = rest[colon + 1..].trim_start();
        let q = rest.chars().next()?;
        if q != '\'' && q != '"' {
            return None;
        }
        let end = rest[1..].find(q)?;
        Some(rest[1..1 + end].to_string())
    };
    let descr = quoted("descr").ok_or("npy header: no descr")?;
    if header.contains("'fortran_order': True") {
        return Err("fortran_order arrays unsupported".into());
    }
    let shape_at = header.find("'shape'").ok_or("npy header: no shape")?;
    let open = header[shape_at..].find('(').ok_or("npy header: bad shape")? + shape_at;
    let close = header[open..].find(')').ok_or("npy header: bad shape")? + open;
    let shape: Vec<usize> = header[open + 1..close]
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.parse::<usize>().map_err(|e| format!("npy shape: {e}")))
        .collect::<Result<_, _>>()?;

    let count: usize = shape.iter().product::<usize>().max(1);
    let payload = &bytes[hstart + hlen..];
    let take = |esize: usize| -> Result<&[u8], String> {
        payload
            .get(..count * esize)
            .ok_or_else(|| format!("npy data truncated: want {} bytes", count * esize))
    };
    let data = match descr.as_str() {
        "<f4" => NpyData::F32(
            take(4)?.chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect(),
        ),
        "<f8" => NpyData::F64(
            take(8)?.chunks_exact(8).map(|c| f64::from_le_bytes(c.try_into().unwrap())).collect(),
        ),
        "<i8" => NpyData::I64(
            take(8)?.chunks_exact(8).map(|c| i64::from_le_bytes(c.try_into().unwrap())).collect(),
        ),
        other => return Err(format!("unsupported npy descr '{other}'")),
    };
    Ok(Npy { shape, data })
}

// ── meta.json ───────────────────────────────────────────────────────────────

/// Calibration block from `meta.json` — the scale constants the feature math
/// uses. Defaults mirror `shadow_train.dataset` so an older meta stays usable.
#[derive(Clone, serde::Deserialize)]
#[allow(non_snake_case)]
pub struct Calibration {
    #[serde(default = "d_ground_y")] pub GROUND_Y: f64,
    #[serde(default = "d_x_scale")] pub X_SCALE: f64,
    #[serde(default = "d_y_scale")] pub Y_SCALE: f64,
    #[serde(default = "d_timer_scale")] pub TIMER_SCALE: f64,
    #[serde(default = "d_anim_scale")] pub ANIM_SCALE: f64,
    #[serde(default = "d_corner_px")] pub CORNER_PX: f64,
    #[serde(default = "d_health_max")] pub HEALTH_MAX: f64,
    #[serde(default = "d_screen_w")] pub SCREEN_W: f64,
    /// Decision period in emulated frames (~7.5 Hz at 60 fps).
    #[serde(default = "d_p")] pub P: u64,
    /// Stacked decision-tick snapshots per feature vector.
    #[serde(default = "d_k")] pub K: usize,
    #[serde(default = "d_hitstun")] pub HITSTUN_RECENT_FRAMES: u64,
}

fn d_ground_y() -> f64 { 216.0 }
fn d_x_scale() -> f64 { 128.0 }
fn d_y_scale() -> f64 { 128.0 }
fn d_timer_scale() -> f64 { 256.0 }
fn d_anim_scale() -> f64 { 64.0 }
fn d_corner_px() -> f64 { 24.0 }
fn d_health_max() -> f64 { 239.0 }
fn d_screen_w() -> f64 { 320.0 }
fn d_p() -> u64 { 8 }
fn d_k() -> usize { 4 }
fn d_hitstun() -> u64 { 20 }

impl Default for Calibration {
    fn default() -> Self {
        serde_json::from_str("{}").unwrap()
    }
}

impl Calibration {
    /// Merge a model's `meta.json` calibration snapshot with the loaded
    /// profile's own `calibration` block: meta wins per-key (it snapshots the
    /// fit-side constants — RECORDER_V3.md §4.3), the profile fills any key
    /// the meta lacks, and the hardcoded literals above are the last-resort
    /// default for a very old meta/profile that maps neither.
    fn merged(
        meta_cal: Option<&std::collections::BTreeMap<String, f64>>,
        prof: &crate::profile::GameProfile,
    ) -> Calibration {
        let get = |key: &str, default: f64| -> f64 {
            meta_cal
                .and_then(|m| m.get(key).copied())
                .or_else(|| prof.calibration(key))
                .unwrap_or(default)
        };
        Calibration {
            GROUND_Y: get("GROUND_Y", d_ground_y()),
            X_SCALE: get("X_SCALE", d_x_scale()),
            Y_SCALE: get("Y_SCALE", d_y_scale()),
            TIMER_SCALE: get("TIMER_SCALE", d_timer_scale()),
            ANIM_SCALE: get("ANIM_SCALE", d_anim_scale()),
            CORNER_PX: get("CORNER_PX", d_corner_px()),
            HEALTH_MAX: get("HEALTH_MAX", d_health_max()),
            SCREEN_W: get("SCREEN_W", d_screen_w()),
            P: get("P", d_p() as f64) as u64,
            K: get("K", d_k() as f64) as usize,
            HITSTUN_RECENT_FRAMES: get("HITSTUN_RECENT_FRAMES", d_hitstun() as f64) as u64,
        }
    }
}

#[derive(serde::Deserialize)]
struct MetaJson {
    feature_names: Vec<String>,
    /// Class vocabularies the model's heads were fitted with. meta.json is
    /// authoritative for a LOADED model (docs/game-profiles.md rule 3);
    /// absent (old models) they default to the canonical asurabld-era lists.
    #[serde(default = "d_move_classes")]
    move_classes: Vec<String>,
    #[serde(default = "d_attack_classes")]
    attack_classes: Vec<String>,
    /// Provenance stamps: which game family / port the model was trained on.
    /// family mismatch = hard error (wrong game's model); port mismatch =
    /// warning (cross-port shadows are a supported experiment).
    #[serde(default)]
    family: Option<String>,
    #[serde(default)]
    port: Option<String>,
    /// Raw snapshot, merged with the profile's own calibration per-key by
    /// [`Calibration::merged`] — NOT deserialized straight into `Calibration`
    /// (that would silently fall back to asurabld-shaped hardcoded defaults
    /// for any key a sparse-feature meta omits, instead of this game's own
    /// profile calibration).
    #[serde(default)]
    calibration: Option<std::collections::BTreeMap<String, f64>>,
    #[serde(default)]
    neutral_cap: Option<f64>,
    // Fit-summary fields for the model card (absent in very old models).
    #[serde(default)]
    n_rounds: Option<u64>,
    #[serde(default)]
    created: Option<String>,
    #[serde(default)]
    bucket_counts: Option<std::collections::BTreeMap<String, u64>>,
    // Matchup key (phase A metas): which chars this model was filtered to.
    // Absent/None = a general model (any me / any opponent).
    #[serde(default)]
    char_filter: Option<u8>,
    #[serde(default)]
    opp_filter: Option<u8>,
}

/// Canonical bucket display order (matches the trainer's coverage report).
const BUCKET_ORDER: [&str; 5] = ["defense", "offense", "air", "corner", "neutral"];

fn d_move_classes() -> Vec<String> {
    MOVE_CLASSES.iter().map(|s| s.to_string()).collect()
}
fn d_attack_classes() -> Vec<String> {
    ATTACK_CLASSES.iter().map(|s| s.to_string()).collect()
}

// ── the kNN policy (mirror of knn.py, soft retrieval) ───────────────────────

pub struct KnnModel {
    pub n: usize,
    pub dims: usize,
    /// Standardized case matrix, row-major n × dims.
    pub x: Vec<f64>,
    pub mu: Vec<f64>,
    pub sd: Vec<f64>,
    pub y_move: Vec<u8>,
    pub y_attack: Vec<u8>,
    pub k: usize,
    pub temperature: f64,
    /// Head sizes — the lengths of the model's move/attack class lists
    /// (meta.json). Nothing here hardcodes 9/6 (game-profiles.md rule 3).
    pub n_move: usize,
    pub n_attack: usize,
}

impl KnnModel {
    /// Build from decoded npz entries, validating shapes. `n_move`/`n_attack`
    /// are the head sizes from the model's class lists; every label must fall
    /// inside them.
    pub fn from_npz(
        mut entries: HashMap<String, Npy>,
        n_move: usize,
        n_attack: usize,
    ) -> Result<KnnModel, String> {
        let mut need = |key: &str| -> Result<Npy, String> {
            entries.remove(key).ok_or_else(|| format!("cases.npz: missing array '{key}'"))
        };
        let x = need("X")?;
        let mu = need("mu")?;
        let sd = need("sd")?;
        let y_move = need("y_move")?;
        let y_attack = need("y_attack")?;
        let k = need("k")?;
        let temperature = need("temperature")?;

        if x.shape.len() != 2 {
            return Err(format!("cases.npz: X must be 2-D, got shape {:?}", x.shape));
        }
        let (n, dims) = (x.shape[0], x.shape[1]);
        for (name, a, want) in [("mu", &mu, dims), ("sd", &sd, dims), ("y_move", &y_move, n), ("y_attack", &y_attack, n)] {
            if a.elem_count() != want {
                return Err(format!(
                    "cases.npz: {name} has {} elements, expected {want}",
                    a.elem_count()
                ));
            }
        }
        let to_labels = |a: &Npy, classes: i64, head: &str| -> Result<Vec<u8>, String> {
            a.as_i64()
                .iter()
                .map(|&v| {
                    if (0..classes).contains(&v) {
                        Ok(v as u8)
                    } else {
                        Err(format!("cases.npz: {head} label {v} out of range 0..{classes}"))
                    }
                })
                .collect()
        };
        let k = k.as_i64()[0];
        if k < 1 {
            return Err(format!("cases.npz: k={k} invalid"));
        }
        Ok(KnnModel {
            n,
            dims,
            x: x.as_f64(),
            mu: mu.as_f64(),
            sd: sd.as_f64(),
            y_move: to_labels(&y_move, n_move as i64, "y_move")?,
            y_attack: to_labels(&y_attack, n_attack as i64, "y_attack")?,
            k: k as usize,
            temperature: temperature.as_f64()[0],
            n_move,
            n_attack,
        })
    }

    /// knn.py `_weighted_neighbors` + `_vote`: probabilities for both heads,
    /// sized `n_move` / `n_attack` from the model's own class lists.
    /// `q_raw` is the UNstandardized stacked feature vector (len == dims).
    pub fn predict_proba(&self, q_raw: &[f64]) -> (Vec<f64>, Vec<f64>) {
        debug_assert_eq!(q_raw.len(), self.dims);
        // Standardize the query, then brute-force distances to all N cases.
        let q: Vec<f64> = (0..self.dims)
            .map(|j| (q_raw[j] - self.mu[j]) / self.sd[j])
            .collect();
        let mut dist: Vec<(f64, usize)> = (0..self.n)
            .map(|i| {
                let row = &self.x[i * self.dims..(i + 1) * self.dims];
                let d2: f64 = row.iter().zip(&q).map(|(a, b)| (a - b) * (a - b)).sum();
                (d2.sqrt(), i)
            })
            .collect();
        // Nearest max(k, WIDE_K), ascending by distance.
        let wide = self.k.max(WIDE_K).min(self.n);
        dist.select_nth_unstable_by(wide - 1, |a, b| a.0.total_cmp(&b.0));
        let near = &mut dist[..wide];
        near.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
        // sigma = the k-th neighbor's distance (min 1e-6).
        let sigma = near[self.k.min(wide) - 1].0.max(1e-6);
        // Per head: weighted class counts → temperature softmax over log-counts.
        let mut cm = vec![0.0f64; self.n_move];
        let mut ca = vec![0.0f64; self.n_attack];
        for &(d, i) in near.iter() {
            let w = (-(d / sigma) * (d / sigma)).exp();
            cm[self.y_move[i] as usize] += w;
            ca[self.y_attack[i] as usize] += w;
        }
        (vote(&cm, self.temperature), vote(&ca, self.temperature))
    }
}

/// knn.py `_vote`: softmax(ln(counts + 1e-9) / T); T <= 0 → argmax one-hot.
/// Length follows `counts` (the head's class count).
fn vote(counts: &[f64], temperature: f64) -> Vec<f64> {
    let mut p = vec![0.0f64; counts.len()];
    if temperature <= 0.0 {
        p[argmax(counts)] = 1.0;
        return p;
    }
    let logits: Vec<f64> = counts.iter().map(|&c| (c + 1e-9).ln() / temperature).collect();
    let m = logits.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let mut sum = 0.0;
    for (i, &l) in logits.iter().enumerate() {
        p[i] = (l - m).exp();
        sum += p[i];
    }
    for v in p.iter_mut() {
        *v /= sum;
    }
    p
}

fn argmax(v: &[f64]) -> usize {
    let mut best = 0;
    for (i, &x) in v.iter().enumerate() {
        if x > v[best] {
            best = i;
        }
    }
    best
}

// ── RNG: tiny xorshift64* (no new deps; rand is not a direct dependency) ────

pub struct XorShift64 {
    state: u64,
}

impl XorShift64 {
    /// Fixed constant mixed with a caller value (the frame count at enable
    /// time) via splitmix64 — deterministic per enable, never Date-based.
    pub fn new(mix: u64) -> Self {
        let mut z = 0x9E37_79B9_7F4A_7C15u64 ^ mix.wrapping_mul(0xBF58_476D_1CE4_E5B9);
        // One splitmix64 round to spread low-entropy seeds.
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        XorShift64 { state: z.max(1) }
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in [0, 1).
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }

    /// Sample an index from a probability vector (cumulative inverse).
    pub fn sample(&mut self, probs: &[f64]) -> usize {
        let r = self.next_f64();
        let mut acc = 0.0;
        for (i, &p) in probs.iter().enumerate() {
            acc += p;
            if r < acc {
                return i;
            }
        }
        probs.len() - 1
    }
}

// ── intent → RETRO mask (SPEC §3c; mirror of runtime.intent_to_mask) ────────

/// True when `list` is the canonical 9-way movement vocabulary the engine's
/// direction logic understands (0 Neutral 1 Forward 2 Back 3 Up 4 Down
/// 5 UpForward 6 UpBack 7 DownForward 8 DownBack).
fn move_classes_are_canonical(list: &[String]) -> bool {
    list.len() == MOVE_CLASSES.len() && list.iter().zip(MOVE_CLASSES).all(|(a, b)| a == b)
}

/// Compile one RETRO button mask per attack class (in the model's
/// `attack_classes` order) from the profile's `attack_chords` table
/// (game-profiles.md rule 4: chords are data). "None" compiles to an empty
/// mask; any other class must have a chord of known RETRO button names.
pub fn compile_attack_masks(attack_classes: &[String]) -> Result<Vec<u16>, String> {
    let prof = crate::profile::current();
    attack_classes
        .iter()
        .map(|class| {
            if class == "None" {
                return Ok(0u16);
            }
            let chord = prof.port.attack_chords.get(class).ok_or_else(|| {
                format!(
                    "attack class '{class}' has no chord in the {} profile's attack_chords",
                    prof.family.family
                )
            })?;
            let mut m = 0u16;
            for name in chord {
                let bit = crate::profile::retro_button_bit(name).ok_or_else(|| {
                    format!("attack chord for '{class}' names unknown RETRO button '{name}'")
                })?;
                m |= 1 << bit;
            }
            Ok(m)
        })
        .collect()
}

/// Directions are engine logic (`s` = facing sign, +1 → Forward is Right);
/// the attack buttons come from `attack_masks` — the per-model table
/// compiled by [`compile_attack_masks`] at load time.
pub fn intent_to_mask(move_class: usize, attack_class: usize, s: i32, attack_masks: &[u16]) -> u16 {
    let mut m = 0u16;
    if matches!(move_class, 3 | 5 | 6) {
        m |= 1 << BIT_UP;
    }
    if matches!(move_class, 4 | 7 | 8) {
        m |= 1 << BIT_DOWN;
    }
    let (fwd, back) = if s > 0 { (BIT_RIGHT, BIT_LEFT) } else { (BIT_LEFT, BIT_RIGHT) };
    if matches!(move_class, 1 | 5 | 7) {
        m |= 1 << fwd;
    }
    if matches!(move_class, 2 | 6 | 8) {
        m |= 1 << back;
    }
    m | attack_masks.get(attack_class).copied().unwrap_or(0)
}

// ── per-tick game state (mirror of runtime.parse_tick over read_addr) ───────

#[derive(Clone, Copy, Default)]
struct Fighter {
    timer: u16,
    anim: u16,
    x: u16,
    y: u16,
    facing: u8,
    health: u8,
    meter: u8,
    meter_max: u8,
}

/// 16-bit read honoring the profile's `memory.endianness` (mirror of
/// `training::rd16`/record.rs's helper): `read_addr` returns little-endian
/// bytes; big-endian guests (68k on asurabld's arcade port) need a swap,
/// little-endian guests (68k on MK2 Genesis, per that profile's own
/// declaration) don't. Never hardcode the swap — the CPU being 68k does not
/// imply the byte order this port's profile was authored against.
fn rd16(ds: &DebugState, addr: u32, little: bool) -> u16 {
    let v = ds.read_addr(addr as usize, 2).unwrap_or(0) as u16;
    if little { v } else { v.swap_bytes() }
}

fn u8g(ds: &DebugState, addr: u32) -> u8 {
    ds.read_addr(addr as usize, 1).unwrap_or(0) as u8
}

/// Read one fighter block. `x` always comes from [`RunnerAddrs`] (universal);
/// `char_id` is read SEPARATELY by [`read_char_id_pair`] (it drives matchup
/// selection, not any scalar feature); everything else here is whatever this
/// model's [`FeatureAddrs`] resolved — absent fields read as 0 (never
/// consulted by [`build_scalars`] unless the model's `feature_names` asked
/// for them).
fn read_fighter(ds: &DebugState, base: u32, x_addr: u32, ra: &RunnerAddrs, o: &FighterOffs) -> Fighter {
    let opt16 = |off: Option<u32>| off.map(|o| rd16(ds, base + o, ra.little)).unwrap_or(0);
    let opt8 = |off: Option<u32>| off.map(|o| u8g(ds, base + o)).unwrap_or(0);
    Fighter {
        timer: opt16(o.timer),
        anim: opt16(o.anim),
        x: rd16(ds, x_addr, ra.little),
        y: opt16(o.y),
        facing: opt8(o.facing),
        health: opt8(o.health),
        meter: opt8(o.meter),
        meter_max: opt8(o.meter_max),
    }
}

#[derive(Clone, Copy)]
struct TickSnapshot {
    block1: Fighter,
    block2: Fighter,
    /// block1's combo landing ON block2 / block2's combo landing ON block1.
    /// (0, 0) when the active model needs no hitstun feature.
    combo_on_b1: u8,
    combo_on_b2: u8,
}

fn read_tick(ds: &DebugState, ra: &RunnerAddrs, fa: &FeatureAddrs) -> TickSnapshot {
    let (combo_on_b1, combo_on_b2) = fa
        .hitstun_globals
        .map(|(g1, g2)| (u8g(ds, g1), u8g(ds, g2)))
        .unwrap_or((0, 0));
    TickSnapshot {
        block1: read_fighter(ds, ra.block1, ra.x1, ra, &fa.offs),
        block2: read_fighter(ds, ra.block2, ra.x2, ra, &fa.offs),
        combo_on_b1,
        combo_on_b2,
    }
}

/// Read just the two `x` values — used for the round-anchor probe before any
/// model's [`FeatureAddrs`] is consulted (x is universal; see [`RunnerAddrs`]).
fn read_x_pair(ds: &DebugState, ra: &RunnerAddrs) -> (u16, u16) {
    (rd16(ds, ra.x1, ra.little), rd16(ds, ra.x2, ra.little))
}

/// Read the two `char_id` values, if the profile maps one.
fn read_char_id_pair(ds: &DebugState, ra: &RunnerAddrs) -> Option<(u8, u8)> {
    ra.char_id_off.map(|o| (u8g(ds, ra.block1 + o), u8g(ds, ra.block2 + o)))
}

// ── hitstun edge tracker (runtime.HitstunTracker, tick-granular) ────────────

#[derive(Default)]
struct HitstunTracker {
    prev: Option<u8>,
    last_change_tick: Option<u64>,
}

impl HitstunTracker {
    fn reset(&mut self) {
        *self = HitstunTracker::default();
    }
    fn update(&mut self, tick: u64, value: u8, window_ticks: u64) -> bool {
        if let Some(p) = self.prev {
            if value != p {
                self.last_change_tick = Some(tick);
            }
        }
        self.prev = Some(value);
        value != 0
            && matches!(self.last_change_tick, Some(t) if tick - t <= window_ticks)
    }
}

// ── the runner ──────────────────────────────────────────────────────────────

/// One fitted model plus everything needed to run and identify it. A runner
/// holds one or many of these (a model SET); per-matchup selection picks the
/// active one at round start.
pub struct LoadedModel {
    model: KnnModel,
    cal: Calibration,
    /// Identity card for the Training panel (published via `shadow_model`).
    pub info: crate::debug::ShadowModelInfo,
    /// The model's OWN class vocabularies (meta.json; canonical defaults for
    /// old metas). Display lookups use these, not the crate-level consts.
    move_classes: Vec<String>,
    attack_classes: Vec<String>,
    /// One RETRO button mask per attack class, compiled from the profile's
    /// `attack_chords` at load ([`compile_attack_masks`]).
    attack_masks: Vec<u16>,
    /// Matchup key from meta.json: (my char, opponent char); None = any.
    me: Option<u8>,
    opp: Option<u8>,
    /// Fit timestamp, for newest-per-key dedup when loading a set.
    created: String,
    /// This model's OWN feature list, in vector order — model-meta-driven,
    /// never a hardcoded 21-scalar assumption (a sparse-feature model like
    /// MK2 Genesis's may declare 13).
    feature_names: Vec<String>,
    /// Fighter-field/global addresses resolved from `feature_names` (only
    /// what this model actually needs — see [`FeatureAddrs::from_profile`]).
    feature_addrs: FeatureAddrs,
}

pub struct ShadowRunner {
    /// The loaded model(s). A single-model load has exactly one entry; a set
    /// load has one per matchup key (newest per key wins).
    library: Vec<LoadedModel>,
    /// Index into `library` currently driving decisions.
    active: usize,
    /// Addresses every model shares, resolved from the profile once at load.
    addrs: RunnerAddrs,
    pub enabled: bool,
    rng: XorShift64,
    // Per-round buffers (runtime.RoundBuffers) — cleared on every gate edge.
    was_live: bool,
    /// `Some(true)` = the bot is block1. Resolved per round by the larger-X
    /// auto-anchor; `None` until the first live frame where the X's differ
    /// (ticking per frame, we can see the gate's very first frame — before
    /// the game has written the fighters' positions — which play.py's 8 Hz
    /// poll effectively never does, so anchoring is deferred until real
    /// positions exist rather than mirrored blindly).
    me_block1: Option<bool>,
    frames_live: u64,
    tick: u64,
    /// Stacked decision-tick scalar vectors — each entry's length is the
    /// ACTIVE model's `feature_names.len()` (model-meta-driven, not a fixed
    /// 21). Safe across a mid-set model switch because switches only ever
    /// happen at round start, before this is filled (`reset_round` clears it).
    stacker: VecDeque<Vec<f32>>,
    me_hitstun: HitstunTracker,
    opp_hitstun: HitstunTracker,
    prev_opp: Option<Fighter>,
    prev_opp_combo: u8,
    last_emitted_mask: u16,
    /// The mask held for the current decision window (re-injected each frame).
    latched_mask: u16,
    /// True once the stacker has filled and a decision has been latched — the
    /// injector only writes port-1 holds from then on, so a not-yet-warmed-up
    /// shadow doesn't stomp other port-1 input sources with zeros.
    driving: bool,
}

impl ShadowRunner {
    /// Load a model directory — either a single model (`cases.npz` +
    /// `meta.json` directly inside) or a SET (a directory of model dirs, e.g.
    /// `shadow/models`). Set loads keep the newest model per matchup key and
    /// pick the right one automatically at every round start. Returns an
    /// ENABLED runner; errors are strings meant for a fatal `--shadow`
    /// startup message (runtime loads report them softly instead).
    pub fn load(dir: &Path) -> Result<ShadowRunner, String> {
        let addrs = RunnerAddrs::from_profile(crate::profile::current())?;
        let mut library: Vec<LoadedModel> = Vec::new();
        if dir.join("cases.npz").is_file() {
            library.push(Self::load_single(dir)?);
        } else {
            // Set: newest model per (me, opp) key; individual failures warn.
            let mut best: std::collections::BTreeMap<(Option<u8>, Option<u8>), LoadedModel> =
                std::collections::BTreeMap::new();
            let entries = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            for e in entries.flatten() {
                let sub = e.path();
                if !sub.join("cases.npz").is_file() {
                    continue;
                }
                match Self::load_single(&sub) {
                    Ok(lm) => {
                        let key = (lm.me, lm.opp);
                        let replace = best
                            .get(&key)
                            .map(|cur| lm.created > cur.created)
                            .unwrap_or(true);
                        if replace {
                            best.insert(key, lm);
                        }
                    }
                    Err(err) => eprintln!("[shadow] set: skipping {}: {err}", sub.display()),
                }
            }
            library.extend(best.into_values());
            if library.is_empty() {
                return Err(format!(
                    "{}: no loadable models (need cases.npz directly inside, or in subdirs)",
                    dir.display()
                ));
            }
            let keys: Vec<String> = library.iter().map(|m| m.info.name.clone()).collect();
            eprintln!(
                "[shadow] set {}: {} model(s) — {} — matchup picked at each round start",
                dir.display(),
                library.len(),
                keys.join(", ")
            );
        }
        // Start on the general model if the set has one, else the first.
        let active = library
            .iter()
            .position(|m| m.me.is_none() && m.opp.is_none())
            .unwrap_or(0);
        Ok(ShadowRunner {
            library,
            active,
            addrs,
            enabled: true,
            rng: XorShift64::new(0),
            was_live: false,
            me_block1: None,
            frames_live: 0,
            tick: 0,
            stacker: VecDeque::new(),
            me_hitstun: HitstunTracker::default(),
            opp_hitstun: HitstunTracker::default(),
            prev_opp: None,
            prev_opp_combo: 0,
            last_emitted_mask: 0,
            latched_mask: 0,
            driving: false,
        })
    }

    /// The active model's identity card (what `shadow_model` publishes).
    pub fn info(&self) -> &crate::debug::ShadowModelInfo {
        &self.library[self.active].info
    }

    /// Per-matchup selection at round start: exact (me, opp) → per-char
    /// (me, any) → (any, opp) → general (any, any) → keep current. Publishes
    /// the switch into `shadow_model` so the panel card follows.
    fn select_model(&mut self, me_char: u8, opp_char: u8, ds: &mut DebugState) {
        if self.library.len() < 2 {
            return;
        }
        let score = |m: &LoadedModel| -> Option<u32> {
            match (m.me, m.opp) {
                (Some(a), Some(b)) if a == me_char && b == opp_char => Some(0),
                (Some(a), None) if a == me_char => Some(1),
                (None, Some(b)) if b == opp_char => Some(2),
                (None, None) => Some(3),
                _ => None,
            }
        };
        let pick = self
            .library
            .iter()
            .enumerate()
            .filter_map(|(i, m)| score(m).map(|s| (s, i)))
            .min();
        if let Some((_, idx)) = pick {
            if idx != self.active {
                self.active = idx;
                eprintln!(
                    "[shadow] matchup c{me_char}-vs-c{opp_char} → model {}",
                    self.library[idx].info.name
                );
                ds.shadow_model = Some(self.library[idx].info.clone());
            }
        }
    }

    /// Load one model directory: `<dir>/cases.npz` + `<dir>/meta.json`,
    /// validating the feature-name contract.
    fn load_single(dir: &Path) -> Result<LoadedModel, String> {
        let prof = crate::profile::current();
        let meta_path = dir.join("meta.json");
        let meta: MetaJson = serde_json::from_str(
            &std::fs::read_to_string(&meta_path)
                .map_err(|e| format!("{}: {e}", meta_path.display()))?,
        )
        .map_err(|e| format!("{}: {e}", meta_path.display()))?;

        // Provenance: another FAMILY's model is the wrong game — hard error.
        // Another PORT of the same family is a supported experiment — warn,
        // and stamp the model card so the panel shows it.
        match &meta.family {
            Some(fam) if *fam != prof.family.family => {
                return Err(format!(
                    "{}: model was trained for family '{fam}' but the loaded profile is \
                     '{}' — refusing to drive the wrong game",
                    meta_path.display(),
                    prof.family.family
                ));
            }
            Some(_) => {}
            None => eprintln!(
                "[shadow] WARNING: {} has no family stamp — cannot verify it matches \
                 '{}'. Pre-stamp models are asurabld-era; restamp its meta.json.",
                meta_path.display(),
                prof.family.family
            ),
        }
        let cross_port = meta
            .port
            .as_ref()
            .filter(|p| **p != prof.port.port)
            .cloned();
        if let Some(mp) = &cross_port {
            eprintln!(
                "[shadow] {}: cross-port model (trained on port '{mp}', running '{}') — \
                 supported experiment; calibration may differ",
                dir.display(),
                prof.port.port
            );
        }

        // The 9-way movement vocabulary is engine logic (intent_to_mask's
        // direction bits); a model with a different move list can't be driven.
        if !move_classes_are_canonical(&meta.move_classes) {
            return Err(format!(
                "{}: move_classes {:?} != the canonical 9-way list {:?} — the runner's \
                 direction logic only understands the latter",
                meta_path.display(),
                meta.move_classes,
                MOVE_CLASSES
            ));
        }
        // Attack chords are data: compile the model's attack classes into
        // RETRO masks via the profile's attack_chords table.
        let attack_masks = compile_attack_masks(&meta.attack_classes)?;

        let npz_path = dir.join("cases.npz");
        let bytes = std::fs::read(&npz_path)
            .map_err(|e| format!("{}: {e}", npz_path.display()))?;
        let model = KnnModel::from_npz(
            parse_npz(&bytes)?,
            meta.move_classes.len(),
            meta.attack_classes.len(),
        )?;

        // Every declared feature must be one this build knows how to compute
        // (RECORDER_V3.md §4.2's canonical list — model.feature_names is any
        // ORDERED SUBSET of it, never a fixed 21; unknown names are refused
        // rather than silently mis-vectorized).
        for name in &meta.feature_names {
            if !SCALAR_FEATURES.contains(&name.as_str()) {
                return Err(format!(
                    "{}: unknown feature '{name}' — this runner build only knows how to \
                     compute {SCALAR_FEATURES:?}",
                    meta_path.display()
                ));
            }
        }
        // Resolve ONLY what this model's own feature list needs from the
        // profile (per-feature requirements, not an all-or-nothing demand for
        // every asurabld-shaped global — see FeatureAddrs::from_profile).
        let feature_addrs = FeatureAddrs::from_profile(prof, &meta.feature_names)
            .map_err(|e| format!("{}: {e}", meta_path.display()))?;

        let cal = Calibration::merged(meta.calibration.as_ref(), prof);
        let want_dims = cal.K * meta.feature_names.len();
        if model.dims != want_dims {
            return Err(format!(
                "cases.npz X has {} feature dims but calibration K={} × {} features {:?} = {want_dims}",
                model.dims,
                cal.K,
                meta.feature_names.len(),
                meta.feature_names,
            ));
        }
        eprintln!(
            "[shadow] loaded {}: {} cases × {} dims, k={}, temperature={}, neutral_cap={} — \
             drives port 1 when enabled (Shift+F5 / 🎯 Training panel)",
            dir.display(),
            model.n,
            model.dims,
            model.k,
            model.temperature,
            meta.neutral_cap.map_or("?".into(), |v| v.to_string()),
        );
        // Model card: canonical bucket order first, any unknown buckets after.
        let mut buckets: Vec<(String, u64)> = Vec::new();
        if let Some(counts) = &meta.bucket_counts {
            for b in BUCKET_ORDER {
                if let Some(n) = counts.get(b) {
                    buckets.push((b.to_string(), *n));
                }
            }
            for (b, n) in counts {
                if !BUCKET_ORDER.contains(&b.as_str()) {
                    buckets.push((b.clone(), *n));
                }
            }
        }
        let mut name = dir
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| dir.display().to_string());
        if let Some(mp) = &cross_port {
            // Panel-visible cross-port stamp (the ShadowModelInfo card shows
            // the name; there is no separate note field).
            name = format!("{name} [cross-port: {mp}]");
        }
        let info = crate::debug::ShadowModelInfo {
            name,
            cases: model.n,
            rounds: meta.n_rounds,
            created: meta.created.clone(),
            buckets,
        };

        Ok(LoadedModel {
            model,
            cal,
            info,
            move_classes: meta.move_classes,
            attack_classes: meta.attack_classes,
            attack_masks,
            me: meta.char_filter,
            opp: meta.opp_filter,
            created: meta.created.clone().unwrap_or_default(),
            feature_names: meta.feature_names,
            feature_addrs,
        })
    }

    /// Toggle at runtime (Shift+F5). Reseeds the RNG from the frame count on
    /// each enable so repeated sessions differ deterministically.
    pub fn toggle(&mut self, frame: u64) {
        self.enabled = !self.enabled;
        if self.enabled {
            self.rng = XorShift64::new(frame);
            eprintln!("[shadow] ON — driving controller port 1 (Shift+F5 to stop)");
        } else {
            self.reset_round();
            self.was_live = false;
            eprintln!("[shadow] OFF");
        }
    }

    fn reset_round(&mut self) {
        self.me_block1 = None;
        self.frames_live = 0;
        self.tick = 0;
        self.stacker.clear();
        self.me_hitstun.reset();
        self.opp_hitstun.reset();
        self.prev_opp = None;
        self.prev_opp_combo = 0;
        self.last_emitted_mask = 0;
        self.latched_mask = 0;
        self.driving = false;
    }

    /// Run one emulated frame. Called from `Frontend::run_frame` after the
    /// bus-window refresh (reads see this frame's snapshot). Decisions happen
    /// every `P` frames while the gate is open; the chosen mask is re-injected
    /// as 2-frame holds every frame in between.
    pub fn tick(&mut self, ds: &mut DebugState, frame: u64) {
        if !self.enabled {
            return;
        }
        let prof = crate::profile::current();
        // The ONE in-fight gate (RECORDER_V3.md §1.2 rule 3 / game-profiles.md):
        // the same evaluator the recorder and Lua `game.controllable()` use —
        // no private composite here. Fully profile-driven (health range,
        // BCD-timer, menu/gate globals, whatever THIS game's gate list says),
        // so a sparse profile like MK2 Genesis's needs nothing this runner
        // doesn't already resolve for it.
        let live = crate::gate::eval_gate(ds, prof);

        if !live {
            // Off-gate: no injection, buffers cleared for the next round
            // (mirrors play.py requirement 4).
            if self.was_live {
                self.reset_round();
                eprintln!("[shadow] round end (gate closed at frame {frame})");
            }
            self.was_live = false;
            return;
        }
        if !self.was_live {
            // Rising edge: fresh round, buffers cleared. The anchor itself may
            // resolve a few frames later (see `me_block1`).
            self.reset_round();
        }
        self.was_live = true;

        // Anchor probe: `x` is universal (every model needs it — RunnerAddrs),
        // so this is read independent of which model is active, BEFORE any
        // matchup switch decides that.
        let (x1, x2) = read_x_pair(ds, &self.addrs);
        if self.me_block1.is_none() {
            if x1 == x2 {
                // Positions not written yet (or a dead heat) — wait a frame.
                return;
            }
            // The BOT is the block with the LARGER X (the mirror of the
            // recorder's p1_block = smaller X).
            self.me_block1 = Some(x1 > x2);
            eprintln!(
                "[shadow] round start — me=block{} (x1={x1} x2={x2})",
                if self.me_block1 == Some(true) { 1 } else { 2 },
            );
            // Matchup selection: chars are valid once positions are — pick
            // the best model for (my char, opponent char) from the set. Only
            // possible when the profile maps `char_id` at all; absent, the
            // set never switches away from its starting model.
            if let Some((c1, c2)) = read_char_id_pair(ds, &self.addrs) {
                let (me_c, opp_c) = if self.me_block1 == Some(true) { (c1, c2) } else { (c2, c1) };
                self.select_model(me_c, opp_c, ds);
            }
        }

        // Read the full per-model feature snapshot AFTER any matchup switch
        // above, so it reflects the model that will actually consume it.
        let fa = self.library[self.active].feature_addrs;
        let snap = read_tick(ds, &self.addrs, &fa);

        if self.frames_live % self.library[self.active].cal.P == 0 {
            self.decide(&snap);
        }
        self.frames_live += 1;

        // Injection: refresh 2-frame holds for the latched mask every frame
        // (training.rs dummy idiom). Only once we're actually driving, so the
        // warm-up ticks don't zero out other port-1 sources.
        if self.driving {
            for i in 0..12 {
                ds.injected_input2[i] = if self.latched_mask >> i & 1 == 1 { 2 } else { 0 };
            }
        }
    }

    /// One ~7.5 Hz decision: build the stacked feature vector exactly as
    /// play.py does, sample (move, attack), latch the mask.
    fn decide(&mut self, snap: &TickSnapshot) {
        let me_is_block1 = self.me_block1 == Some(true);
        let (me_now, opp_now) = if me_is_block1 {
            (snap.block1, snap.block2)
        } else {
            (snap.block2, snap.block1)
        };
        let (me_combo_now, opp_combo_now) = if me_is_block1 {
            (snap.combo_on_b1, snap.combo_on_b2)
        } else {
            (snap.combo_on_b2, snap.combo_on_b1)
        };

        // Opponent read one decision tick stale (runtime.py approximation #1) —
        // resolved BEFORE `s` because the no-`facing`-field fallback below
        // needs this same stale read (dataset.py `_decisions_for_round`
        // order: `s = sign(opp["x"] - me["x"])` uses the STALE opp).
        let opp_lagged = self.prev_opp.unwrap_or(opp_now);
        let opp_combo_lagged = self.prev_opp_combo;

        let has_facing = self.library[self.active].feature_addrs.offs.facing.is_some();
        let s: i32 = if has_facing {
            if me_now.facing == 1 { 1 } else { -1 }
        } else {
            // RECORDER_V3.md §4.2 facing fallback: no `facing` field mapped
            // (e.g. MK2 Genesis) → s = sign(opp.x - me.x). With this s,
            // dist_x collapses to |Δx| and fwd/back holds become
            // position-relative, exactly as the contract note says.
            if opp_lagged.x as i32 - me_now.x as i32 >= 0 { 1 } else { -1 }
        };

        // Holds from the bot's own last EMITTED mask (not the intent class).
        let (fwd_bit, back_bit) = if s > 0 { (BIT_RIGHT, BIT_LEFT) } else { (BIT_LEFT, BIT_RIGHT) };
        let fwd_hold = (self.last_emitted_mask >> fwd_bit & 1) as f64;
        let back_hold = (self.last_emitted_mask >> back_bit & 1) as f64;

        let window_ticks = (self.library[self.active].cal.HITSTUN_RECENT_FRAMES / self.library[self.active].cal.P).max(1);
        let me_hit = self.me_hitstun.update(self.tick, me_combo_now, window_ticks);
        let opp_hit = self.opp_hitstun.update(self.tick, opp_combo_lagged, window_ticks);

        let k = self.library[self.active].cal.K;
        let scal = build_scalars(
            &self.library[self.active].feature_names,
            &self.library[self.active].cal,
            &me_now,
            &opp_lagged,
            s,
            fwd_hold,
            back_hold,
            me_hit,
            opp_hit,
        );
        if self.stacker.len() == k {
            self.stacker.pop_front();
        }
        self.stacker.push_back(scal);

        if self.stacker.len() == k {
            // Oldest → newest concatenation (dataset.build stacking order).
            let q: Vec<f64> = self
                .stacker
                .iter()
                .flat_map(|s| s.iter().map(|&v| v as f64))
                .collect();
            let (pm, pa) = self.library[self.active].model.predict_proba(&q);
            let mv = self.rng.sample(&pm);
            let at = self.rng.sample(&pa);
            let mask = intent_to_mask(mv, at, s, &self.library[self.active].attack_masks);
            self.latched_mask = mask;
            self.last_emitted_mask = mask;
            self.driving = true;
            if self.tick < 12 || self.tick % 75 == 0 {
                let lm = &self.library[self.active];
                let dist_x = lm.feature_names.iter().position(|n| n == "dist_x").map(|i| self.stacker.back().unwrap()[i]);
                eprintln!(
                    "[shadow] tick={:4} me=block{} dist_x={:?} s={:+} move={} attack={} mask={:#05x}",
                    self.tick,
                    if me_is_block1 { 1 } else { 2 },
                    dist_x,
                    s,
                    lm.move_classes[mv],
                    lm.attack_classes[at],
                    mask
                );
            }
        }

        self.prev_opp = Some(opp_now);
        self.prev_opp_combo = opp_combo_now;
        self.tick += 1;
    }
}

/// Canonical class vocabularies — the movement contract the engine's
/// direction logic requires, and the DEFAULT lists for metas that predate
/// per-model class lists. Display and sizing prefer the loaded model's own
/// lists; these consts stay for external users.
pub const MOVE_CLASSES: [&str; 9] = [
    "Neutral", "Forward", "Back", "Up", "Down", "UpForward", "UpBack", "DownForward", "DownBack",
];
pub const ATTACK_CLASSES: [&str; 6] = ["None", "Light", "Medium", "Heavy", "Launcher", "Toss"];

/// The scalar vector in the MODEL'S OWN `feature_names` order (mirror of
/// `dataset.py`'s per-feature formulas in `_decisions_for_round` — see the
/// RECORDER_V3.md §4.2 availability table this mirrors). Sparse by
/// construction: a model that never declared `me_anim` never asks for it
/// here, so a profile that doesn't map `anim` is never consulted for it
/// either (this is what makes a 13-feature MK2 Genesis model loadable next to
/// a 21-feature asurabld one — see `FeatureAddrs::from_profile`).
#[allow(clippy::too_many_arguments)]
fn build_scalars(
    feature_names: &[String],
    cal: &Calibration,
    me: &Fighter,
    opp: &Fighter,
    s: i32,
    fwd_hold: f64,
    back_hold: f64,
    me_hitstun: bool,
    opp_hitstun: bool,
) -> Vec<f32> {
    let sf = s as f64;
    let airborne = |f: &Fighter| if cal.GROUND_Y - f.y as f64 > 4.0 { 1.0 } else { 0.0 };
    let height = |f: &Fighter| (cal.GROUND_Y - f.y as f64).max(0.0) / cal.Y_SCALE;
    feature_names
        .iter()
        .map(|name| {
            let v: f64 = match name.as_str() {
                "dist_x" => sf * (opp.x as f64 - me.x as f64) / cal.X_SCALE,
                "dy" => (opp.y as f64 - me.y as f64) / cal.Y_SCALE,
                "me_airborne" => airborne(me),
                "me_height" => height(me),
                "me_fwd_hold" => fwd_hold,
                "me_back_hold" => back_hold,
                "me_anim" => me.anim as f64 / cal.ANIM_SCALE,
                "me_timer" => me.timer as f64 / cal.TIMER_SCALE,
                "opp_airborne" => airborne(opp),
                "opp_height" => height(opp),
                "opp_anim" => opp.anim as f64 / cal.ANIM_SCALE,
                "opp_timer" => opp.timer as f64 / cal.TIMER_SCALE,
                "facing_sign" => sf,
                "me_health" => me.health as f64 / cal.HEALTH_MAX,
                "opp_health" => opp.health as f64 / cal.HEALTH_MAX,
                "health_lead" => (me.health as f64 - opp.health as f64) / cal.HEALTH_MAX,
                "me_meter" => me.meter as f64 / (me.meter_max as f64).max(1.0),
                "opp_meter" => opp.meter as f64 / (opp.meter_max as f64).max(1.0),
                "me_hitstun" => if me_hitstun { 1.0 } else { 0.0 },
                "opp_hitstun" => if opp_hitstun { 1.0 } else { 0.0 },
                "me_corner" => {
                    if me.x as f64 <= cal.CORNER_PX || me.x as f64 >= cal.SCREEN_W - cal.CORNER_PX {
                        1.0
                    } else {
                        0.0
                    }
                }
                other => unreachable!(
                    "feature '{other}' unknown to build_scalars — should have been \
                     rejected at load (load_single validates every feature_names entry \
                     against SCALAR_FEATURES)"
                ),
            };
            // float32, like runtime.scalars_to_vector — the query then
            // standardizes in f64 exactly as numpy upcasts.
            v as f32
        })
        .collect()
}

// ── tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // Attack-button bits, for asserting what the asurabld chord table must
    // compile to (Light=B, Medium=A, Heavy=Y).
    const BIT_B: u16 = 0;
    const BIT_Y: u16 = 1;
    const BIT_A: u16 = 8;

    fn goat_v2() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("shadow/models/asurabld/goat-v2")
    }

    fn load_goat_model() -> KnnModel {
        let bytes = std::fs::read(goat_v2().join("cases.npz")).expect(
            "shadow/models/asurabld/goat-v2/cases.npz missing — the npz golden tests need the real model",
        );
        KnnModel::from_npz(parse_npz(&bytes).unwrap(), 9, 6).unwrap()
    }

    fn approx(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol
    }

    /// Reference values extracted with shadow/train/.venv python:
    ///   d = numpy.load('shadow/models/asurabld/goat-v2/cases.npz')
    ///   X float32 (13405, 84); mu/sd float32 (84,); y_* int64; k=15; T=1.0
    ///   X[0][:4]  = [ 0.09305416,  0.04422662, -0.3448887 , -0.28220278]
    ///   mu[:4]    = [ 0.8746433 , -0.89317596,  0.10630362,  0.05048781]
    ///   sd[:4]    = [ 4.4535265 , 20.19544   ,  0.30822587,  0.17890613]
    ///   y_move[:8] = zeros, y_attack[:8] = zeros
    #[test]
    fn npz_reader_decodes_goat_v2() {
        crate::profile::init_for_tests();
        let bytes = std::fs::read(goat_v2().join("cases.npz")).unwrap();
        let entries = parse_npz(&bytes).unwrap();
        let x = &entries["X"];
        assert_eq!(x.shape, vec![13405, 84]);
        let xv = x.as_f64();
        for (got, want) in xv[..4]
            .iter()
            .zip([0.09305416f64, 0.04422662, -0.3448887, -0.28220278])
        {
            assert!(approx(*got, want, 1e-7), "X[0][:4] {got} vs {want}");
        }
        let mu = entries["mu"].as_f64();
        let sd = entries["sd"].as_f64();
        assert_eq!(mu.len(), 84);
        assert_eq!(sd.len(), 84);
        for (got, want) in mu[..4].iter().zip([0.8746433f64, -0.89317596, 0.10630362, 0.05048781]) {
            assert!(approx(*got, want, 1e-6), "mu {got} vs {want}");
        }
        for (got, want) in sd[..4].iter().zip([4.4535265f64, 20.19544, 0.30822587, 0.17890613]) {
            assert!(approx(*got, want, 1e-5), "sd {got} vs {want}");
        }
        assert_eq!(&entries["y_move"].as_i64()[..8], &[0i64; 8]);
        assert_eq!(&entries["y_attack"].as_i64()[..8], &[0i64; 8]);
        assert_eq!(entries["k"].as_i64(), vec![15]);
        assert_eq!(entries["temperature"].as_f64(), vec![1.0]);

        // Model card lifted from meta.json (Training panel display).
        let runner = ShadowRunner::load(&goat_v2()).unwrap();
        assert_eq!(runner.library.len(), 1);
        assert_eq!(runner.info().name, "goat-v2");
        assert_eq!(runner.info().cases, 13405);
        assert_eq!(runner.info().rounds, Some(112));
        assert!(runner.info().created.as_deref().unwrap_or("").starts_with("2026-08-25"));
        // Canonical bucket order, counts as fitted.
        let names: Vec<&str> = runner.info().buckets.iter().map(|(b, _)| b.as_str()).collect();
        assert_eq!(names, ["defense", "offense", "air", "corner", "neutral"]);
        assert_eq!(runner.info().buckets[0].1, 183);

        let model = KnnModel::from_npz(entries, 9, 6).unwrap();
        assert_eq!(model.n, 13405);
        assert_eq!(model.dims, 84);
        assert_eq!(model.k, 15);
        assert_eq!(model.temperature, 1.0);
        assert_eq!((model.n_move, model.n_attack), (9, 6));
    }

    /// Golden vote values from the python policy (KnnPolicy.load + the same
    /// queries, float64):
    ///   q1 = zeros(84); q2 = mu; q3 = X[42]*sd + mu (destandardized case 42).
    #[test]
    fn predict_proba_matches_python_goldens() {
        let m = load_goat_model();

        // q1: all-zero raw query.
        let (pm, pa) = m.predict_proba(&vec![0.0; 84]);
        let pm_want = [
            0.8819146913404625, 0.010146720048809782, 0.04102696239452997,
            2.8972707120448238e-11, 0.04796547001708566, 2.8972707120448238e-11,
            2.8972707120448238e-11, 0.009479666389491463, 0.009466489722702299,
        ];
        let pa_want = [
            0.8844648567241598, 0.057492731481462764, 0.048221414700181466,
            0.009820997036250669, 2.897270712296646e-11, 2.897270712296646e-11,
        ];
        for (g, w) in pm.iter().zip(pm_want) {
            assert!(approx(*g, w, 1e-6), "q1 pm {g} vs {w}");
        }
        for (g, w) in pa.iter().zip(pa_want) {
            assert!(approx(*g, w, 1e-6), "q1 pa {g} vs {w}");
        }

        // q2: the mean vector (standardizes to ~0).
        let (pm, pa) = m.predict_proba(&m.mu.clone());
        let pm_want = [
            0.9643838434359729, 0.023736074100639583, 0.011880082282065229,
            3.022032951361444e-11, 3.022032951361444e-11, 3.022032951361444e-11,
            3.022032951361444e-11, 3.022032951361444e-11, 3.022032951361444e-11,
        ];
        let pa_want = [
            0.9471422160427613, 0.019910089366969506, 3.022032951635425e-11,
            0.010141894028126146, 3.022032951635425e-11, 0.02280580050170236,
        ];
        for (g, w) in pm.iter().zip(pm_want) {
            assert!(approx(*g, w, 1e-6), "q2 pm {g} vs {w}");
        }
        for (g, w) in pa.iter().zip(pa_want) {
            assert!(approx(*g, w, 1e-6), "q2 pa {g} vs {w}");
        }

        // q3: case 42, destandardized — its own distance is exactly 0.
        let q3: Vec<f64> = (0..84).map(|j| m.x[42 * 84 + j] * m.sd[j] + m.mu[j]).collect();
        let (pm, pa) = m.predict_proba(&q3);
        let pm_want = [
            0.5561158591216293, 0.17690842990742722, 0.006990364110644373,
            0.06737419846481917, 0.036053933514531136, 0.04685359683356165,
            3.894817359307461e-11, 0.10970361796949084, 3.894817359307461e-11,
        ];
        let pa_want = [
            0.4508569219048671, 0.3813052607543869, 0.1200549933159376,
            0.041708822512647334, 0.006074001473213005, 3.8948173597625485e-11,
        ];
        for (g, w) in pm.iter().zip(pm_want) {
            assert!(approx(*g, w, 1e-6), "q3 pm {g} vs {w}");
        }
        for (g, w) in pa.iter().zip(pa_want) {
            assert!(approx(*g, w, 1e-6), "q3 pa {g} vs {w}");
        }
    }

    #[test]
    fn vote_argmax_when_temperature_zero() {
        let p = vote(&[0.5, 3.0, 0.1], 0.0);
        assert_eq!(p, [0.0, 1.0, 0.0]);
    }

    /// SPEC §3c intent → RETRO bits, both facings, full class matrix —
    /// through the compiled-chords path. The masks are pinned to the exact
    /// values the pre-profile hardcoded match produced.
    #[test]
    fn intent_to_mask_matrix() {
        crate::profile::init_for_tests();
        let am = compile_attack_masks(&d_attack_classes()).unwrap();
        let b = |bit: u16| 1u16 << bit;
        // Directions, s = +1 (Forward = Right).
        assert_eq!(intent_to_mask(0, 0, 1, &am), 0);
        assert_eq!(intent_to_mask(1, 0, 1, &am), b(BIT_RIGHT));
        assert_eq!(intent_to_mask(2, 0, 1, &am), b(BIT_LEFT));
        assert_eq!(intent_to_mask(3, 0, 1, &am), b(BIT_UP));
        assert_eq!(intent_to_mask(4, 0, 1, &am), b(BIT_DOWN));
        assert_eq!(intent_to_mask(5, 0, 1, &am), b(BIT_UP) | b(BIT_RIGHT));
        assert_eq!(intent_to_mask(6, 0, 1, &am), b(BIT_UP) | b(BIT_LEFT));
        assert_eq!(intent_to_mask(7, 0, 1, &am), b(BIT_DOWN) | b(BIT_RIGHT));
        assert_eq!(intent_to_mask(8, 0, 1, &am), b(BIT_DOWN) | b(BIT_LEFT));
        // Directions, s = -1 (Forward = Left).
        assert_eq!(intent_to_mask(1, 0, -1, &am), b(BIT_LEFT));
        assert_eq!(intent_to_mask(2, 0, -1, &am), b(BIT_RIGHT));
        assert_eq!(intent_to_mask(5, 0, -1, &am), b(BIT_UP) | b(BIT_LEFT));
        assert_eq!(intent_to_mask(6, 0, -1, &am), b(BIT_UP) | b(BIT_RIGHT));
        assert_eq!(intent_to_mask(7, 0, -1, &am), b(BIT_DOWN) | b(BIT_LEFT));
        assert_eq!(intent_to_mask(8, 0, -1, &am), b(BIT_DOWN) | b(BIT_RIGHT));
        // Attacks: B=Light, A=Medium, Y=Heavy, Launcher={B,A}, Toss={B,A,Y}.
        assert_eq!(intent_to_mask(0, 1, 1, &am), b(BIT_B));
        assert_eq!(intent_to_mask(0, 2, 1, &am), b(BIT_A));
        assert_eq!(intent_to_mask(0, 3, 1, &am), b(BIT_Y));
        assert_eq!(intent_to_mask(0, 4, 1, &am), b(BIT_B) | b(BIT_A));
        assert_eq!(intent_to_mask(0, 5, 1, &am), b(BIT_B) | b(BIT_A) | b(BIT_Y));
        // Chord: DownForward + Heavy at s = -1.
        assert_eq!(intent_to_mask(7, 3, -1, &am), b(BIT_DOWN) | b(BIT_LEFT) | b(BIT_Y));
    }

    /// The asurabld profile's attack_chords table must compile to EXACTLY the
    /// legacy hardcoded masks — this is the load-time table `decide` uses.
    #[test]
    fn chord_compilation_matches_legacy_masks() {
        crate::profile::init_for_tests();
        let b = |bit: u16| 1u16 << bit;
        let masks = compile_attack_masks(&d_attack_classes()).unwrap();
        assert_eq!(
            masks,
            vec![
                0,                                   // None
                b(BIT_B),                            // Light
                b(BIT_A),                            // Medium
                b(BIT_Y),                            // Heavy
                b(BIT_B) | b(BIT_A),                 // Launcher
                b(BIT_B) | b(BIT_A) | b(BIT_Y),      // Toss
            ]
        );
        // Unknown class (no chord in the profile) is a load error, not a 0.
        assert!(compile_attack_masks(&["Fireball".to_string()]).is_err());
        // The canonical move list passes the load-time assertion; a foreign
        // one does not.
        assert!(move_classes_are_canonical(&d_move_classes()));
        assert!(!move_classes_are_canonical(&["Neutral".to_string()]));
    }

    /// Head sizes follow the class lists, not compiled 9/6 constants: a
    /// synthetic model with 4 attack classes yields a 4-wide attack head.
    #[test]
    fn head_sizes_follow_meta_class_lists() {
        let mk = |shape: Vec<usize>, data: NpyData| Npy { shape, data };
        let entries = |y_attack: Vec<i64>| -> HashMap<String, Npy> {
            let mut e = HashMap::new();
            e.insert("X".into(), mk(vec![4, 2], NpyData::F32(vec![0.0, 0.0, 1.0, 1.0, 2.0, 2.0, 3.0, 3.0])));
            e.insert("mu".into(), mk(vec![2], NpyData::F32(vec![0.0, 0.0])));
            e.insert("sd".into(), mk(vec![2], NpyData::F32(vec![1.0, 1.0])));
            e.insert("y_move".into(), mk(vec![4], NpyData::I64(vec![0, 1, 2, 8])));
            e.insert("y_attack".into(), mk(vec![4], NpyData::I64(y_attack)));
            e.insert("k".into(), mk(vec![], NpyData::I64(vec![2])));
            e.insert("temperature".into(), mk(vec![], NpyData::F64(vec![1.0])));
            e
        };
        let m = KnnModel::from_npz(entries(vec![0, 1, 2, 3]), 9, 4).unwrap();
        assert_eq!((m.n_move, m.n_attack), (9, 4));
        let (pm, pa) = m.predict_proba(&[0.5, 0.5]);
        assert_eq!(pm.len(), 9);
        assert_eq!(pa.len(), 4);
        for p in [&pm, &pa] {
            assert!((p.iter().sum::<f64>() - 1.0).abs() < 1e-12, "distribution sums to 1");
            assert!(p.iter().all(|&v| (0.0..=1.0).contains(&v)));
        }
        // A label outside the declared head is rejected at load.
        let err = KnnModel::from_npz(entries(vec![0, 1, 2, 4]), 9, 4)
            .err()
            .expect("label 4 must be rejected under a 4-class attack head");
        assert!(err.contains("out of range"), "{err}");
    }

    #[test]
    fn hitstun_tracker_requires_recent_change_not_just_nonzero() {
        let mut t = HitstunTracker::default();
        let w = 2; // window in ticks
        // First sighting of a nonzero value is NOT hitstun (no observed change).
        assert!(!t.update(0, 3, w));
        // Value changes at tick 1 → active while within the window…
        assert!(t.update(1, 4, w));
        assert!(t.update(2, 4, w));
        assert!(t.update(3, 4, w));
        // …then expires (last change at tick 1, window 2 < 4-1).
        assert!(!t.update(4, 4, w));
        // Zero is never hitstun, even right after a change.
        assert!(!t.update(5, 0, w));
    }

    #[test]
    fn xorshift_sampling_is_deterministic_and_in_range() {
        let mut a = XorShift64::new(1234);
        let mut b = XorShift64::new(1234);
        for _ in 0..100 {
            let p = [0.2, 0.5, 0.3];
            let (sa, sb) = (a.sample(&p), b.sample(&p));
            assert_eq!(sa, sb);
            assert!(sa < 3);
        }
        // Degenerate one-hot always picks the hot class.
        let mut c = XorShift64::new(7);
        for _ in 0..50 {
            assert_eq!(c.sample(&[0.0, 0.0, 1.0, 0.0]), 2);
        }
    }

    #[test]
    fn build_scalars_matches_hand_computed_row() {
        let cal = Calibration::default();
        let me = Fighter {
            timer: 512, anim: 128, x: 84, y: 216, facing: 1,
            health: 239, meter: 32, meter_max: 64,
        };
        let opp = Fighter {
            timer: 256, anim: 64, x: 232, y: 200, facing: 0,
            health: 120, meter: 0, meter_max: 64,
        };
        let s = 1; // me.facing == 1
        let feats: Vec<String> = SCALAR_FEATURES.iter().map(|s| s.to_string()).collect();
        let v = build_scalars(&feats, &cal, &me, &opp, s, 1.0, 0.0, false, true);
        let want: [f32; 21] = [
            (232.0 - 84.0) / 128.0,   // dist_x
            (200.0 - 216.0) / 128.0,  // dy
            0.0,                      // me_airborne (216-216 = 0, not > 4)
            0.0,                      // me_height
            1.0,                      // me_fwd_hold
            0.0,                      // me_back_hold
            2.0,                      // me_anim 128/64
            2.0,                      // me_timer 512/256
            1.0,                      // opp_airborne (216-200 = 16 > 4)
            16.0 / 128.0,             // opp_height
            1.0,                      // opp_anim
            1.0,                      // opp_timer
            1.0,                      // facing_sign
            1.0,                      // me_health 239/239
            120.0 / 239.0,            // opp_health
            119.0 / 239.0,            // health_lead
            0.5,                      // me_meter
            0.0,                      // opp_meter
            0.0,                      // me_hitstun
            1.0,                      // opp_hitstun
            0.0,                      // me_corner (84 in 24..296)
        ];
        for (i, (g, w)) in v.iter().zip(want).enumerate() {
            assert!((g - w).abs() < 1e-6, "scalar {i} ({}): {g} vs {w}", SCALAR_FEATURES[i]);
        }
        // Corner: x = 20 (<= 24) and x = 300 (>= 296) both flag.
        let mut cme = me;
        cme.x = 20;
        let corner_i = feats.iter().position(|f| f == "me_corner").unwrap();
        assert_eq!(build_scalars(&feats, &cal, &cme, &opp, s, 0.0, 0.0, false, false)[corner_i], 1.0);
        cme.x = 300;
        assert_eq!(build_scalars(&feats, &cal, &cme, &opp, s, 0.0, 0.0, false, false)[corner_i], 1.0);
    }

    /// RECORDER_V3.md §4.2's availability table, mirrored: a SPARSE feature
    /// list (genesis-smoke-v0's 13 features — no anim/timer/meter/hitstun)
    /// assembles a shorter, differently-ordered-from-the-full-list vector,
    /// and the facing fallback (`s = sign(opp.x - me.x)`) applies since MK2
    /// Genesis maps no `facing` field.
    #[test]
    fn build_scalars_sparse_feature_set_matches_hand_computed_row() {
        let cal = Calibration {
            GROUND_Y: 121.0, X_SCALE: 128.0, Y_SCALE: 128.0, TIMER_SCALE: 256.0,
            ANIM_SCALE: 64.0, CORNER_PX: 24.0, HEALTH_MAX: 120.0, SCREEN_W: 320.0,
            P: 8, K: 4, HITSTUN_RECENT_FRAMES: 20,
        };
        let me = Fighter { x: 84, y: 121, health: 100, ..Default::default() };
        let opp = Fighter { x: 232, y: 105, health: 60, ..Default::default() };
        // No `facing` field on this profile → s = sign(opp.x - me.x) = +1.
        let s = 1;
        let feats: Vec<String> = [
            "dist_x", "dy", "me_airborne", "me_height", "me_fwd_hold", "me_back_hold",
            "opp_airborne", "opp_height", "facing_sign", "me_health", "opp_health",
            "health_lead", "me_corner",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let v = build_scalars(&feats, &cal, &me, &opp, s, 1.0, 0.0, false, false);
        assert_eq!(v.len(), 13, "13 features, no more no less");
        let want: [f32; 13] = [
            (232.0 - 84.0) / 128.0,  // dist_x
            (105.0 - 121.0) / 128.0, // dy
            0.0,                     // me_airborne (121-121=0, not >4)
            0.0,                     // me_height
            1.0,                     // me_fwd_hold
            0.0,                     // me_back_hold
            1.0,                     // opp_airborne (121-105=16 > 4)
            16.0 / 128.0,            // opp_height
            1.0,                     // facing_sign
            100.0 / 120.0,           // me_health
            60.0 / 120.0,            // opp_health
            (100.0 - 60.0) / 120.0,  // health_lead
            0.0,                     // me_corner (84 in 24..296)
        ];
        for (i, (g, w)) in v.iter().zip(want).enumerate() {
            assert!((g - w).abs() < 1e-6, "sparse scalar {i} ({}): {g} vs {w}", feats[i]);
        }
    }

    /// Model-meta-driven requirement resolution (RECORDER_V3.md §4.2): the
    /// genesis-smoke-v0 shape (13 sparse features, no hitstun) resolves
    /// cleanly against the real MK2 Genesis profile, which maps no
    /// `hitstun_sources` at all — a model that DID need hitstun must fail,
    /// naming both the feature and the missing profile piece, without ever
    /// touching the global `crate::profile::current()` (these load a
    /// standalone `GameProfile`, so this test can't race other tests' global
    /// asurabld profile install).
    #[test]
    fn feature_addrs_resolve_per_model_meta_not_all_or_nothing() {
        let genesis = crate::profile::GameProfile::load(Path::new("library/mk2/genesis"))
            .expect("library/mk2/genesis.profile.json loads");

        let sparse: Vec<String> = [
            "dist_x", "dy", "me_airborne", "me_height", "me_fwd_hold", "me_back_hold",
            "opp_airborne", "opp_height", "facing_sign", "me_health", "opp_health",
            "health_lead", "me_corner",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let fa = FeatureAddrs::from_profile(&genesis, &sparse)
            .expect("genesis-smoke-v0's feature list needs nothing this profile lacks");
        // No `facing` field on this profile — the fallback applies.
        assert!(fa.offs.facing.is_none());
        // None of these features were requested — must NOT be required.
        assert!(fa.offs.anim.is_none());
        assert!(fa.offs.timer.is_none());
        assert!(fa.offs.meter.is_none());
        assert!(fa.hitstun_globals.is_none());
        // y/health WERE requested and ARE mapped by this profile.
        assert!(fa.offs.y.is_some());
        assert!(fa.offs.health.is_some());

        // A hitstun-featured meta on the SAME profile (which has no
        // hitstun_sources) must fail, naming the feature and the missing
        // piece — not a generic "profile: shadow runner needs global
        // 'combo_on_b2'" that doesn't say WHY.
        let mut with_hitstun = sparse.clone();
        with_hitstun.push("me_hitstun".to_string());
        with_hitstun.push("opp_hitstun".to_string());
        let err = FeatureAddrs::from_profile(&genesis, &with_hitstun)
            .err()
            .expect("genesis profile has no hitstun_sources — must fail");
        assert!(err.contains("me_hitstun"), "{err}");
        assert!(err.contains("hitstun_sources"), "{err}");

        // A model needing `me_anim` similarly fails naming 'anim' + the
        // feature that wants it (genesis profile maps no 'anim' field).
        let mut with_anim = sparse.clone();
        with_anim.push("me_anim".to_string());
        let err = FeatureAddrs::from_profile(&genesis, &with_anim).err().expect("no anim field");
        assert!(err.contains("me_anim"), "{err}");
        assert!(err.contains("'anim'"), "{err}");
    }

    /// The runner's in-flight gate is `gate::eval_gate` — the SAME
    /// evaluator the recorder and Lua `game.controllable()` use, no private
    /// composite (RECORDER_V3.md §1.2 rule 3). End-to-end through a real
    /// `ShadowRunner::load`ed model against a synthetic asurabld memory
    /// image: closed gate → no anchor/injection; opening it lets the
    /// round-start anchor (larger-x = bot) resolve, and after enough decision
    /// ticks the shadow starts driving port 1.
    #[test]
    fn tick_gate_is_training_eval_gate_and_resolves_anchor() {
        crate::profile::init_for_tests();
        let prof = crate::profile::current();
        let mut runner = ShadowRunner::load(&goat_v2()).unwrap();

        let mut ds = DebugState::new();
        assert!(ds.install_bus_window(crate::debug::BusWindowCfg {
            name: "Work RAM".into(),
            addr: 0x400000,
            len: 0x8000,
            interval: 1,
            flags: crate::libretro::RETRO_MEMDESC_SYSTEM_RAM,
        }));
        // Big-endian guest: to make the game "see" value V at a 2-byte
        // field, write V.swap_bytes() (write_addr's own bytes are read back
        // unswapped by read_addr; the swap happens in this module's `rd16`).
        let wbig16 = |ds: &mut DebugState, addr: u32, v: u16| {
            ds.write_addr(addr as usize, 2, v.swap_bytes() as u32);
        };
        let wbig8 = |ds: &mut DebugState, addr: u32, v: u8| {
            ds.write_addr(addr as usize, 1, v as u32);
        };

        let (b1, b2) = (prof.block1(), prof.block2());
        let (x_off, _) = prof.field_off("x").unwrap();
        let (health_off, _) = prof.field_off("health").unwrap();

        // Gate closed: round_over nonzero.
        wbig16(&mut ds, prof.global("round_over").unwrap(), 1);
        wbig16(&mut ds, prof.global("abort").unwrap(), 0);
        wbig16(&mut ds, prof.global("match_end").unwrap(), 0);
        wbig8(&mut ds, prof.global("char_select").unwrap(), 0);
        wbig8(&mut ds, prof.global("round_timer").unwrap(), 0x30);
        wbig8(&mut ds, b1 + health_off, 200);
        wbig8(&mut ds, b2 + health_off, 150);
        wbig16(&mut ds, b1 + x_off, 200);
        wbig16(&mut ds, b2 + x_off, 50);

        runner.tick(&mut ds, 0);
        assert!(runner.me_block1.is_none(), "gate closed — no anchor yet");
        assert!(!runner.driving);

        // Open the gate (round_over -> 0): same eval_gate the recorder uses.
        wbig16(&mut ds, prof.global("round_over").unwrap(), 0);
        assert!(crate::gate::eval_gate(&ds, prof), "gate must now read open");

        // Drive enough ticks (P * K, plus headroom) for the anchor to
        // resolve and a full K-deep stack to produce a decision.
        for f in 0..(8 * 4 + 8) {
            runner.tick(&mut ds, f as u64);
        }
        assert_eq!(runner.me_block1, Some(true), "block1 has the larger x (200 > 50)");
        assert!(runner.driving, "a full K-stack must have produced a decision by now");

        // Closing the gate again ends the round and clears the anchor.
        wbig16(&mut ds, prof.global("round_over").unwrap(), 1);
        runner.tick(&mut ds, 1000);
        assert!(runner.me_block1.is_none());
        assert!(!runner.driving);
    }

    /// meta.json provenance: wrong family = hard error; same family on a
    /// different port = accepted with a panel-visible cross-port stamp.
    #[test]
    fn provenance_family_errors_and_port_warns() {
        crate::profile::init_for_tests();
        let base = std::env::temp_dir().join(format!("shadow_prov_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let make = |name: &str, edit: &dyn Fn(&mut serde_json::Value)| -> PathBuf {
            let d = base.join(name);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::copy(goat_v2().join("cases.npz"), d.join("cases.npz")).unwrap();
            let mut meta: serde_json::Value = serde_json::from_str(
                &std::fs::read_to_string(goat_v2().join("meta.json")).unwrap(),
            )
            .unwrap();
            edit(&mut meta);
            std::fs::write(d.join("meta.json"), meta.to_string()).unwrap();
            d
        };

        // Wrong family: refused outright.
        let wrong = make("wrong-family", &|m| {
            m["family"] = serde_json::json!("sf2ce");
        });
        let err = ShadowRunner::load(&wrong).err().expect("family mismatch must fail");
        assert!(err.contains("family 'sf2ce'"), "{err}");

        // Same family, other port: loads, warns, stamps the model card name.
        let cross = make("cross-port", &|m| {
            m["family"] = serde_json::json!("asurabld");
            m["port"] = serde_json::json!("console");
        });
        let runner = ShadowRunner::load(&cross).unwrap();
        assert_eq!(runner.info().name, "cross-port [cross-port: console]");

        // Matching stamps: loads with a clean name.
        let exact = make("exact", &|m| {
            m["family"] = serde_json::json!("asurabld");
            m["port"] = serde_json::json!("arcade");
        });
        let runner = ShadowRunner::load(&exact).unwrap();
        assert_eq!(runner.info().name, "exact");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn set_load_dedups_and_selects_per_matchup() {
        crate::profile::init_for_tests();
        // Build a temp set from the tracked goat-v2 artifacts: a general
        // model plus a synthetic (me=1, opp=7) matchup model with the same
        // cases but an edited meta.
        let set = std::env::temp_dir().join(format!("shadow_set_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&set);
        for (name, me, opp) in [("general", None, None), ("goat-vs-rosemary", Some(1), Some(7))] {
            let d = set.join(name);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::copy(goat_v2().join("cases.npz"), d.join("cases.npz")).unwrap();
            let mut meta: serde_json::Value = serde_json::from_str(
                &std::fs::read_to_string(goat_v2().join("meta.json")).unwrap(),
            )
            .unwrap();
            meta["char_filter"] = serde_json::json!(me);
            meta["opp_filter"] = serde_json::json!(opp);
            std::fs::write(d.join("meta.json"), meta.to_string()).unwrap();
        }
        let mut runner = ShadowRunner::load(&set).unwrap();
        assert_eq!(runner.library.len(), 2);
        // Starts on the general model.
        assert_eq!(runner.info().name, "general");

        let mut ds = DebugState::new();
        // Known matchup → the per-matchup model; unknown → back to general.
        runner.select_model(1, 7, &mut ds);
        assert_eq!(runner.info().name, "goat-vs-rosemary");
        assert_eq!(ds.shadow_model.as_ref().unwrap().name, "goat-vs-rosemary");
        runner.select_model(1, 3, &mut ds);
        assert_eq!(runner.info().name, "general");
        // Same matchup again is a no-op (stays where it is).
        runner.select_model(1, 3, &mut ds);
        assert_eq!(runner.info().name, "general");
        let _ = std::fs::remove_dir_all(&set);
    }
}
