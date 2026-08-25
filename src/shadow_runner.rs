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
//! `shadow/models/goat-v2` model.
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

// RETRO joypad bit indices (dataset.py / src/mcp/server.rs order).
const BIT_B: u16 = 0; // Light
const BIT_Y: u16 = 1; // Heavy
const BIT_UP: u16 = 4;
const BIT_DOWN: u16 = 5;
const BIT_LEFT: u16 = 6;
const BIT_RIGHT: u16 = 7;
const BIT_A: u16 = 8; // Medium

// Game addresses (library/asurabld/asurabld.md; same map as record.rs /
// training.rs / shadow_train.runtime).
const BLOCK1: u32 = 0x403798;
const BLOCK2: u32 = 0x40454C;
const COMBO_ON_B2: u32 = 0x4041E7; // block1's combo landing ON block2
const COMBO_ON_B1: u32 = 0x40470B; // block2's combo landing ON block1
const ROUND_OVER: u32 = 0x40646E;
const ABORT: u32 = 0x403678;
const MATCH_END: u32 = 0x402A32;
const ROUND_TIMER: u32 = 0x40000A; // BCD seconds

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

#[derive(serde::Deserialize)]
struct MetaJson {
    feature_names: Vec<String>,
    #[serde(default)]
    calibration: Option<Calibration>,
    #[serde(default)]
    neutral_cap: Option<f64>,
    // Fit-summary fields for the model card (absent in very old models).
    #[serde(default)]
    n_rounds: Option<u64>,
    #[serde(default)]
    created: Option<String>,
    #[serde(default)]
    bucket_counts: Option<std::collections::BTreeMap<String, u64>>,
}

/// Canonical bucket display order (matches the trainer's coverage report).
const BUCKET_ORDER: [&str; 5] = ["defense", "offense", "air", "corner", "neutral"];

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
}

impl KnnModel {
    /// Build from decoded npz entries, validating shapes.
    pub fn from_npz(mut entries: HashMap<String, Npy>) -> Result<KnnModel, String> {
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
            y_move: to_labels(&y_move, 9, "y_move")?,
            y_attack: to_labels(&y_attack, 6, "y_attack")?,
            k: k as usize,
            temperature: temperature.as_f64()[0],
        })
    }

    /// knn.py `_weighted_neighbors` + `_vote`: probabilities for both heads.
    /// `q_raw` is the UNstandardized stacked feature vector (len == dims).
    pub fn predict_proba(&self, q_raw: &[f64]) -> ([f64; 9], [f64; 6]) {
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
        let mut cm = [0.0f64; 9];
        let mut ca = [0.0f64; 6];
        for &(d, i) in near.iter() {
            let w = (-(d / sigma) * (d / sigma)).exp();
            cm[self.y_move[i] as usize] += w;
            ca[self.y_attack[i] as usize] += w;
        }
        (vote(&cm, self.temperature), vote(&ca, self.temperature))
    }
}

/// knn.py `_vote`: softmax(ln(counts + 1e-9) / T); T <= 0 → argmax one-hot.
fn vote<const C: usize>(counts: &[f64; C], temperature: f64) -> [f64; C] {
    let mut p = [0.0f64; C];
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

/// Move classes: 0 Neutral 1 Forward 2 Back 3 Up 4 Down 5 UpForward 6 UpBack
/// 7 DownForward 8 DownBack. Attacks: 0 None 1 Light(B) 2 Medium(A)
/// 3 Heavy(Y) 4 Launcher(B+A) 5 Toss(B+A+Y). `s` = facing sign
/// (+1 → Forward is Right).
pub fn intent_to_mask(move_class: usize, attack_class: usize, s: i32) -> u16 {
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
    match attack_class {
        1 => m |= 1 << BIT_B,
        2 => m |= 1 << BIT_A,
        3 => m |= 1 << BIT_Y,
        4 => m |= (1 << BIT_B) | (1 << BIT_A),
        5 => m |= (1 << BIT_B) | (1 << BIT_A) | (1 << BIT_Y),
        _ => {}
    }
    m
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

/// `read_addr` returns little-endian bytes; the 68k stores big-endian — swap
/// (same as record.rs's u16be helper).
fn u16be(ds: &DebugState, addr: u32) -> u16 {
    (ds.read_addr(addr as usize, 2).unwrap_or(0) as u16).swap_bytes()
}

fn u8g(ds: &DebugState, addr: u32) -> u8 {
    ds.read_addr(addr as usize, 1).unwrap_or(0) as u8
}

fn read_fighter(ds: &DebugState, base: u32) -> Fighter {
    Fighter {
        timer: u16be(ds, base),
        anim: u16be(ds, base + 0x12),
        x: u16be(ds, base + 0x54),
        y: u16be(ds, base + 0x56),
        facing: u8g(ds, base + 0x61),
        health: u8g(ds, base + 0x177),
        meter: u8g(ds, base + 0x17B),
        meter_max: u8g(ds, base + 0x17F),
    }
}

#[derive(Clone, Copy)]
struct TickSnapshot {
    block1: Fighter,
    block2: Fighter,
    combo_on_b1: u8,
    combo_on_b2: u8,
    round_over: u16,
    abort: u16,
    match_end: u16,
    timer_bcd: u8,
}

fn read_tick(ds: &DebugState) -> TickSnapshot {
    TickSnapshot {
        block1: read_fighter(ds, BLOCK1),
        block2: read_fighter(ds, BLOCK2),
        combo_on_b1: u8g(ds, COMBO_ON_B1),
        combo_on_b2: u8g(ds, COMBO_ON_B2),
        round_over: u16be(ds, ROUND_OVER),
        abort: u16be(ds, ABORT),
        match_end: u16be(ds, MATCH_END),
        timer_bcd: u8g(ds, ROUND_TIMER),
    }
}

fn timer_bcd_valid(t: u8) -> bool {
    t != 0 && (t >> 4) <= 9 && (t & 0xF) <= 9
}

/// The composite in-fight gate (record.rs `controllable` / runtime.py
/// `is_controllable`): hop flags clear, both healths live, clock is BCD.
fn is_controllable(snap: &TickSnapshot, health_max: u8) -> bool {
    let healthy = |f: &Fighter| (1..=health_max).contains(&f.health);
    snap.round_over == 0
        && snap.abort == 0
        && snap.match_end == 0
        && healthy(&snap.block1)
        && healthy(&snap.block2)
        && timer_bcd_valid(snap.timer_bcd)
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

pub struct ShadowRunner {
    model: KnnModel,
    cal: Calibration,
    /// Identity card for the Training panel (published via `shadow_model`).
    pub info: crate::debug::ShadowModelInfo,
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
    stacker: VecDeque<[f32; 21]>,
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
    /// Load `<dir>/cases.npz` + `<dir>/meta.json`, validate the feature-name
    /// contract, and return an ENABLED runner. Errors are strings meant for a
    /// fatal `--shadow` startup message.
    pub fn load(dir: &Path) -> Result<ShadowRunner, String> {
        let npz_path = dir.join("cases.npz");
        let bytes = std::fs::read(&npz_path)
            .map_err(|e| format!("{}: {e}", npz_path.display()))?;
        let model = KnnModel::from_npz(parse_npz(&bytes)?)?;

        let meta_path = dir.join("meta.json");
        let meta: MetaJson = serde_json::from_str(
            &std::fs::read_to_string(&meta_path)
                .map_err(|e| format!("{}: {e}", meta_path.display()))?,
        )
        .map_err(|e| format!("{}: {e}", meta_path.display()))?;

        if meta.feature_names[..] != SCALAR_FEATURES[..] {
            return Err(format!(
                "feature_names mismatch — this build computes {:?} but {} declares {:?}; \
                 retrain the model or update src/shadow_runner.rs to match",
                SCALAR_FEATURES,
                meta_path.display(),
                meta.feature_names,
            ));
        }
        let cal = meta.calibration.unwrap_or_default();
        let want_dims = cal.K * SCALAR_FEATURES.len();
        if model.dims != want_dims {
            return Err(format!(
                "cases.npz X has {} feature dims but calibration K={} × {} scalars = {want_dims}",
                model.dims,
                cal.K,
                SCALAR_FEATURES.len()
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
        let info = crate::debug::ShadowModelInfo {
            name: dir
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| dir.display().to_string()),
            cases: model.n,
            rounds: meta.n_rounds,
            created: meta.created.clone(),
            buckets,
        };

        Ok(ShadowRunner {
            model,
            cal,
            info,
            enabled: true,
            rng: XorShift64::new(0),
            was_live: false,
            me_block1: None,
            frames_live: 0,
            tick: 0,
            stacker: VecDeque::with_capacity(4),
            me_hitstun: HitstunTracker::default(),
            opp_hitstun: HitstunTracker::default(),
            prev_opp: None,
            prev_opp_combo: 0,
            last_emitted_mask: 0,
            latched_mask: 0,
            driving: false,
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
        let snap = read_tick(ds);
        let live = is_controllable(&snap, self.cal.HEALTH_MAX as u8);

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

        if self.me_block1.is_none() {
            if snap.block1.x == snap.block2.x {
                // Positions not written yet (or a dead heat) — wait a frame.
                return;
            }
            // The BOT is the block with the LARGER X (the mirror of the
            // recorder's p1_block = smaller X).
            self.me_block1 = Some(snap.block1.x > snap.block2.x);
            eprintln!(
                "[shadow] round start — me=block{} (x1={} x2={})",
                if self.me_block1 == Some(true) { 1 } else { 2 },
                snap.block1.x,
                snap.block2.x
            );
        }

        if self.frames_live % self.cal.P == 0 {
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

        let s: i32 = if me_now.facing == 1 { 1 } else { -1 };

        // Opponent read one decision tick stale (runtime.py approximation #1).
        let opp_lagged = self.prev_opp.unwrap_or(opp_now);
        let opp_combo_lagged = self.prev_opp_combo;

        // Holds from the bot's own last EMITTED mask (not the intent class).
        let (fwd_bit, back_bit) = if s > 0 { (BIT_RIGHT, BIT_LEFT) } else { (BIT_LEFT, BIT_RIGHT) };
        let fwd_hold = (self.last_emitted_mask >> fwd_bit & 1) as f64;
        let back_hold = (self.last_emitted_mask >> back_bit & 1) as f64;

        let window_ticks = (self.cal.HITSTUN_RECENT_FRAMES / self.cal.P).max(1);
        let me_hit = self.me_hitstun.update(self.tick, me_combo_now, window_ticks);
        let opp_hit = self.opp_hitstun.update(self.tick, opp_combo_lagged, window_ticks);

        let scal = build_scalars(&self.cal, &me_now, &opp_lagged, s, fwd_hold, back_hold, me_hit, opp_hit);
        if self.stacker.len() == self.cal.K {
            self.stacker.pop_front();
        }
        self.stacker.push_back(scal);

        if self.stacker.len() == self.cal.K {
            // Oldest → newest concatenation (dataset.build stacking order).
            let q: Vec<f64> = self
                .stacker
                .iter()
                .flat_map(|s| s.iter().map(|&v| v as f64))
                .collect();
            let (pm, pa) = self.model.predict_proba(&q);
            let mv = self.rng.sample(&pm);
            let at = self.rng.sample(&pa);
            let mask = intent_to_mask(mv, at, s);
            self.latched_mask = mask;
            self.last_emitted_mask = mask;
            self.driving = true;
            if self.tick < 12 || self.tick % 75 == 0 {
                eprintln!(
                    "[shadow] tick={:4} me=block{} dist_x={:+.3} s={:+} move={} attack={} mask={:#05x}",
                    self.tick,
                    if me_is_block1 { 1 } else { 2 },
                    scal[0],
                    s,
                    MOVE_CLASSES[mv],
                    ATTACK_CLASSES[at],
                    mask
                );
            }
        }

        self.prev_opp = Some(opp_now);
        self.prev_opp_combo = opp_combo_now;
        self.tick += 1;
    }
}

pub const MOVE_CLASSES: [&str; 9] = [
    "Neutral", "Forward", "Back", "Up", "Down", "UpForward", "UpBack", "DownForward", "DownBack",
];
pub const ATTACK_CLASSES: [&str; 6] = ["None", "Light", "Medium", "Heavy", "Launcher", "Toss"];

/// The §1a scalar vector in SCALAR_FEATURES order (runtime.build_scalars).
#[allow(clippy::too_many_arguments)]
fn build_scalars(
    cal: &Calibration,
    me: &Fighter,
    opp: &Fighter,
    s: i32,
    fwd_hold: f64,
    back_hold: f64,
    me_hitstun: bool,
    opp_hitstun: bool,
) -> [f32; 21] {
    let sf = s as f64;
    let airborne = |f: &Fighter| if cal.GROUND_Y - f.y as f64 > 4.0 { 1.0 } else { 0.0 };
    let height = |f: &Fighter| (cal.GROUND_Y - f.y as f64).max(0.0) / cal.Y_SCALE;
    let v: [f64; 21] = [
        sf * (opp.x as f64 - me.x as f64) / cal.X_SCALE,          // dist_x
        (opp.y as f64 - me.y as f64) / cal.Y_SCALE,               // dy
        airborne(me),                                             // me_airborne
        height(me),                                               // me_height
        fwd_hold,                                                 // me_fwd_hold
        back_hold,                                                // me_back_hold
        me.anim as f64 / cal.ANIM_SCALE,                          // me_anim
        me.timer as f64 / cal.TIMER_SCALE,                        // me_timer
        airborne(opp),                                            // opp_airborne
        height(opp),                                              // opp_height
        opp.anim as f64 / cal.ANIM_SCALE,                         // opp_anim
        opp.timer as f64 / cal.TIMER_SCALE,                       // opp_timer
        sf,                                                       // facing_sign
        me.health as f64 / cal.HEALTH_MAX,                        // me_health
        opp.health as f64 / cal.HEALTH_MAX,                       // opp_health
        (me.health as f64 - opp.health as f64) / cal.HEALTH_MAX,  // health_lead
        me.meter as f64 / (me.meter_max as f64).max(1.0),         // me_meter
        opp.meter as f64 / (opp.meter_max as f64).max(1.0),       // opp_meter
        if me_hitstun { 1.0 } else { 0.0 },                       // me_hitstun
        if opp_hitstun { 1.0 } else { 0.0 },                      // opp_hitstun
        if me.x as f64 <= cal.CORNER_PX || me.x as f64 >= cal.SCREEN_W - cal.CORNER_PX {
            1.0
        } else {
            0.0
        },                                                        // me_corner
    ];
    // float32, like runtime.scalars_to_vector — the query then standardizes in
    // f64 exactly as numpy upcasts.
    let mut out = [0.0f32; 21];
    for (o, x) in out.iter_mut().zip(v) {
        *o = x as f32;
    }
    out
}

// ── tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn goat_v2() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("shadow/models/goat-v2")
    }

    fn load_goat_model() -> KnnModel {
        let bytes = std::fs::read(goat_v2().join("cases.npz")).expect(
            "shadow/models/goat-v2/cases.npz missing — the npz golden tests need the real model",
        );
        KnnModel::from_npz(parse_npz(&bytes).unwrap()).unwrap()
    }

    fn approx(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol
    }

    /// Reference values extracted with shadow/train/.venv python:
    ///   d = numpy.load('shadow/models/goat-v2/cases.npz')
    ///   X float32 (13405, 84); mu/sd float32 (84,); y_* int64; k=15; T=1.0
    ///   X[0][:4]  = [ 0.09305416,  0.04422662, -0.3448887 , -0.28220278]
    ///   mu[:4]    = [ 0.8746433 , -0.89317596,  0.10630362,  0.05048781]
    ///   sd[:4]    = [ 4.4535265 , 20.19544   ,  0.30822587,  0.17890613]
    ///   y_move[:8] = zeros, y_attack[:8] = zeros
    #[test]
    fn npz_reader_decodes_goat_v2() {
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
        assert_eq!(runner.info.name, "goat-v2");
        assert_eq!(runner.info.cases, 13405);
        assert_eq!(runner.info.rounds, Some(112));
        assert!(runner.info.created.as_deref().unwrap_or("").starts_with("2026-08-25"));
        // Canonical bucket order, counts as fitted.
        let names: Vec<&str> = runner.info.buckets.iter().map(|(b, _)| b.as_str()).collect();
        assert_eq!(names, ["defense", "offense", "air", "corner", "neutral"]);
        assert_eq!(runner.info.buckets[0].1, 183);

        let model = KnnModel::from_npz(entries).unwrap();
        assert_eq!(model.n, 13405);
        assert_eq!(model.dims, 84);
        assert_eq!(model.k, 15);
        assert_eq!(model.temperature, 1.0);
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

    /// SPEC §3c intent → RETRO bits, both facings, full class matrix.
    #[test]
    fn intent_to_mask_matrix() {
        let b = |bit: u16| 1u16 << bit;
        // Directions, s = +1 (Forward = Right).
        assert_eq!(intent_to_mask(0, 0, 1), 0);
        assert_eq!(intent_to_mask(1, 0, 1), b(BIT_RIGHT));
        assert_eq!(intent_to_mask(2, 0, 1), b(BIT_LEFT));
        assert_eq!(intent_to_mask(3, 0, 1), b(BIT_UP));
        assert_eq!(intent_to_mask(4, 0, 1), b(BIT_DOWN));
        assert_eq!(intent_to_mask(5, 0, 1), b(BIT_UP) | b(BIT_RIGHT));
        assert_eq!(intent_to_mask(6, 0, 1), b(BIT_UP) | b(BIT_LEFT));
        assert_eq!(intent_to_mask(7, 0, 1), b(BIT_DOWN) | b(BIT_RIGHT));
        assert_eq!(intent_to_mask(8, 0, 1), b(BIT_DOWN) | b(BIT_LEFT));
        // Directions, s = -1 (Forward = Left).
        assert_eq!(intent_to_mask(1, 0, -1), b(BIT_LEFT));
        assert_eq!(intent_to_mask(2, 0, -1), b(BIT_RIGHT));
        assert_eq!(intent_to_mask(5, 0, -1), b(BIT_UP) | b(BIT_LEFT));
        assert_eq!(intent_to_mask(6, 0, -1), b(BIT_UP) | b(BIT_RIGHT));
        assert_eq!(intent_to_mask(7, 0, -1), b(BIT_DOWN) | b(BIT_LEFT));
        assert_eq!(intent_to_mask(8, 0, -1), b(BIT_DOWN) | b(BIT_RIGHT));
        // Attacks: B=Light, A=Medium, Y=Heavy, Launcher={B,A}, Toss={B,A,Y}.
        assert_eq!(intent_to_mask(0, 1, 1), b(BIT_B));
        assert_eq!(intent_to_mask(0, 2, 1), b(BIT_A));
        assert_eq!(intent_to_mask(0, 3, 1), b(BIT_Y));
        assert_eq!(intent_to_mask(0, 4, 1), b(BIT_B) | b(BIT_A));
        assert_eq!(intent_to_mask(0, 5, 1), b(BIT_B) | b(BIT_A) | b(BIT_Y));
        // Chord: DownForward + Heavy at s = -1.
        assert_eq!(intent_to_mask(7, 3, -1), b(BIT_DOWN) | b(BIT_LEFT) | b(BIT_Y));
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
        let v = build_scalars(&cal, &me, &opp, s, 1.0, 0.0, false, true);
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
        assert_eq!(build_scalars(&cal, &cme, &opp, s, 0.0, 0.0, false, false)[20], 1.0);
        cme.x = 300;
        assert_eq!(build_scalars(&cal, &cme, &opp, s, 0.0, 0.0, false, false)[20], 1.0);
    }

    #[test]
    fn gate_and_anchor_semantics() {
        let f = |health: u8, x: u16| Fighter { health, x, ..Default::default() };
        let snap = TickSnapshot {
            block1: f(0xEF, 84),
            block2: f(0x80, 232),
            combo_on_b1: 0,
            combo_on_b2: 0,
            round_over: 0,
            abort: 0,
            match_end: 0,
            timer_bcd: 0x85,
        };
        assert!(is_controllable(&snap, 0xEF));
        // Any hop flag, dead health, or a non-BCD clock closes the gate.
        assert!(!is_controllable(&TickSnapshot { round_over: 1, ..snap }, 0xEF));
        assert!(!is_controllable(&TickSnapshot { abort: 1, ..snap }, 0xEF));
        assert!(!is_controllable(&TickSnapshot { match_end: 1, ..snap }, 0xEF));
        assert!(!is_controllable(&TickSnapshot { block1: f(0, 84), ..snap }, 0xEF));
        assert!(!is_controllable(&TickSnapshot { block2: f(0xF0, 232), ..snap }, 0xEF));
        assert!(!is_controllable(&TickSnapshot { timer_bcd: 0x3A, ..snap }, 0xEF));
        assert!(!is_controllable(&TickSnapshot { timer_bcd: 0, ..snap }, 0xEF));
    }
}
