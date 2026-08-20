# Wave 0B — Implementation Map (frontend changes 1–3)

Branch: `feature/execution-map`. All line numbers verified against the current
tree (read 2026-08-20). This is a plan only — no code was changed. Wave-1 agents
implement it. Every write path that matters for change 1 was inventoried by grep
(`write_addr` has exactly two live callers; the Watch panel writes nothing
directly — the freeze re-apply loop is the sole watch write path; Lua has no
memory write).

Key architectural fact that shapes all three changes: **`DebugState` (in
`src/debug/mod.rs`) has NO handle to `RetroCore`.** The core lives on `Frontend`
(`src/frontend.rs`). Anything that must touch the live 68k bus (Sek reads/writes)
has to run on the emulation thread inside `Frontend`, and cross the
`Arc<Mutex<DebugState>>` boundary via a queue — exactly the existing pattern used
by `pending_bus_windows`, `save_busmap`, and `pending_lua`.

---

## Change 1 — Sek write path (the keystone)

### The bug, precisely
Bus-window regions are synthesized `MemoryRegion`s whose `ptr` points into an
owned `Box<[u8]>` snapshot (`DebugState.bus_buffers[i]`, allocated in
`install_bus_window`, `debug/mod.rs:688-704`). Every frame,
`Frontend::refresh_bus_windows` (`frontend.rs:299-352`) overwrites that Box from
the live bus via `core.sek_read_block`. `DebugState::write_addr`
(`debug/mod.rs:811-834`) resolves a bus-window address to that Box and writes it —
so the write lands in the snapshot and is clobbered by the next frame's refresh
(`frontend.rs:699`). Net: on fbalpha2012 (no libretro memory map — every region
is a bus window) `write_memory` and `freeze` are **silent no-ops**.

`SekWriteByte/Word/LongFn` are declared (`libretro.rs:696-698`) but never wired,
and there is no `sek_write_block`.

### Write-path inventory (what actually calls write_addr)
- `Frontend::capture_cpu_state` freeze re-apply loop — `frontend.rs:598-600`
  (`for (addr,len,value) in freeze_writes { ds.write_addr(...) }`). `ds` is a
  `&mut` guard. This is the ONLY watch/freeze write path (the Watch panel
  `debug/panels/watch.rs:119-121` only toggles the `frozen` bool + clears
  `frozen_value`; the actual write is this loop).
- MCP `write_memory` — `server.rs:1031` (`ds.write_addr`), `ds` is `&mut`.
- MCP `freeze` — `server.rs:1056-1089` does NOT write directly; it appends/updates
  a `Watch{frozen:true,...}`, which the freeze re-apply loop above then writes each
  frame. So fixing the re-apply loop's write fixes freeze too.
- No Lua write binding exists (`memory.write*` absent — confirmed by grep).

Both live `write_addr` callers already hold a mutable guard, so widening the
signature to `&mut self` is safe.

### The delta

**(1a) `RetroCore::sek_write_block` — `src/libretro.rs`, new method next to
`sek_read_block` (after line 543).** Mirror the read path's guard trio:

```rust
/// Write `bytes` to the live 68k bus starting at `addr` via the core's
/// exported SekWriteByte (fbalpha2012). Byte-wise (no long packing) so it is
/// endianness-symmetric with sek_read_block: byte i goes to bus addr+i, the
/// same position sek_read_block reads it back into. Returns false when the
/// core does not export the guarded Sek API (same probe as sek_read_block).
pub fn sek_write_block(&self, addr: u32, bytes: &[u8]) -> bool {
    if bytes.is_empty() { return true; }
    unsafe {
        if let Ok(initted) = self.library.get::<Symbol<*const u8>>(b"DebugCPU_SekInitted") {
            let p: *const u8 = **initted;
            if !p.is_null() && *p == 0 { return false; }
        }
        let write_byte = match self.library.get::<Symbol<SekWriteByteFn>>(b"_Z12SekWriteBytejh") {
            Ok(f) => f, Err(_) => return false,
        };
        let get_active = match self.library.get::<Symbol<SekGetActiveFn>>(b"_Z12SekGetActivev") {
            Ok(f) => f, Err(_) => return false,
        };
        let open  = match self.library.get::<Symbol<SekOpenFn>>(b"_Z7SekOpeni")  { Ok(f)=>f, Err(_)=>return false };
        let close = match self.library.get::<Symbol<SekCloseFn>>(b"_Z8SekClosev") { Ok(f)=>f, Err(_)=>return false };
        let opened_here = get_active() < 0;
        if opened_here { open(0); }
        for (i, b) in bytes.iter().enumerate() {
            write_byte(addr.wrapping_add(i as u32), *b);
        }
        if opened_here { close(); }
        true
    }
}
```

- Use byte writes only. `sek_read_block` uses long reads for speed (~50k
  bytes/frame); writes are a handful of bytes per freeze, so byte-wise is fine
  and sidesteps the big-endian long-packing question entirely.
- **RISK / must-verify:** the mangled name `_Z12SekWriteBytejh` is derived, not
  observed (SekWriteByte(unsigned int, unsigned char) → `j`,`h`). Confirm before
  trusting: `nm -D <fbalpha2012.dylib/.so> | grep -i SekWrite`. If FBA exports
  only word/long writers, fall back to `_Z12SekWriteWordjt` /
  `_Z12SekWriteLongjj` and pack. The FFI typedefs already exist at
  `libretro.rs:696-698`.

**(1b) `DebugState.pending_bus_writes` queue — `src/debug/mod.rs`.**
Add field to the struct (near `pending_bus_windows`, ~`debug/mod.rs:468`):
```rust
/// Bus-window writes queued for the emulation thread to push to the live 68k
/// bus via sek_write_block (DebugState can't reach the core). Same handoff
/// pattern as pending_bus_windows. (addr, little-endian bytes).
pub pending_bus_writes: Vec<(u32, Vec<u8>)>,
```
Init `pending_bus_writes: Vec::new()` in `DebugState::new` (~`debug/mod.rs:627`).

**(1c) Route `write_addr` for bus-window regions — `debug/mod.rs:811-834`.**
Change signature to `&mut self` and branch on whether the containing region is a
bus window (name is in `self.bus_windows`):

```rust
pub fn write_addr(&mut self, addr: usize, len: usize, value: u32) -> bool {
    let len = len.min(4);
    // Resolve region + whether it's a bus window WITHOUT holding a borrow across
    // the later self.pending_bus_writes.push (borrow-checker: copy out first).
    let mut hit: Option<(*mut u8, bool)> = None;
    for region in &self.memory_regions {
        if region.host_ptr_for_addr(addr).is_none() { continue; }
        if region.is_readonly() { return false; }
        let Some(cptr) = region.safe_host_ptr(addr, len) else { continue; };
        let is_bus = self.bus_windows.iter().any(|w| w.name == region.name);
        hit = Some((cptr as *mut u8, is_bus));
        break;
    }
    let Some((ptr, is_bus)) = hit else { return false; };
    let mut bytes = [0u8; 4];
    unsafe { for k in 0..len { let b = ((value >> (8*k)) & 0xFF) as u8; *ptr.add(k) = b; bytes[k] = b; } }
    if is_bus {
        // Also poked the snapshot Box above so a same-lock read_addr sees the
        // value this frame; queue the REAL bus write for the emu thread.
        self.pending_bus_writes.push((addr as u32, bytes[..len].to_vec()));
    }
    true
}
```
Note the borrow dance: the `is_bus` check borrows `self.bus_windows` while
iterating `self.memory_regions` (both immutable — OK), but the
`pending_bus_writes.push` needs `&mut self`, so the region borrow must end first
(hence copying `ptr`/`is_bus`/`bytes` into locals before the push). Writing the
Box first keeps `read_addr` (which reads the Box) consistent within the same lock
for callers that read-after-write (e.g. `freeze` capturing then re-reading).

**(1d) `Frontend::drain_bus_writes` — `src/frontend.rs`, new method.**
```rust
/// Push queued bus-window writes to the live 68k bus. Takes the queue under a
/// brief lock, then does the FFI unlocked (same discipline as refresh_bus_windows).
fn drain_bus_writes(&mut self) {
    if self.bus_bridge_ok == Some(false) { return; }
    let writes: Vec<(u32, Vec<u8>)> = match self.debug_state.try_lock() {
        Ok(mut ds) => std::mem::take(&mut ds.pending_bus_writes),
        Err(_) => return,
    };
    for (addr, bytes) in writes {
        self.core.sek_write_block(addr, &bytes);
    }
}
```

**(1e) Ordering in `run_frame` — `frontend.rs:689-699`.** Insert the drain
BETWEEN `capture_cpu_state()` (`:696`, which enqueues freeze writes) and
`refresh_bus_windows(None)` (`:699`, which re-snapshots):
```
self.core.run(); self.frame_count += 1;
self.capture_cpu_state();     // :696  freeze re-apply → pending_bus_writes
self.drain_bus_writes();      // NEW   pending_bus_writes → live bus
self.refresh_bus_windows(None); // :699 snapshot now reflects the writes
```
This is why freezes stick: the game's own frame code already ran (`core.run`),
`capture_cpu_state` re-writes the frozen value AFTER it, `drain_bus_writes` puts
that value on the live bus, and the refresh reads it back so the UI/MCP snapshot
agrees. `write_memory` from the MCP thread lands in `pending_bus_writes` and is
drained on the next frame (a ≤1-frame deferral — note in the tool's return that
the write is "queued to the live bus").

### Endianness
`write_addr` writes `value`'s little-endian bytes to ascending addresses;
`sek_write_block` writes those same bytes to ascending bus addresses;
`sek_read_block` reads ascending bus bytes back into ascending Box positions;
`read_addr` re-reads them little-endian. Byte-wise symmetry means a written value
round-trips exactly. The pre-existing "everything is read LE, 68k words look
byte-swapped, use WatchFormat::U16BE to compensate" quirk is neither fixed nor
worsened — it is preserved.

### Risks / gotchas
- Mangled symbol name (see 1b). Single biggest project risk of all three changes.
- Writing to a **write-only I/O bus window** (asurabld video regs $8C0000, priority
  $8E0000, tilebank $A00000 — `asurabld.md` §Memory map) would perturb the machine.
  Bus windows are *supposed* to be RAM-only; the freeze/write targets are Work RAM
  ($400000). Document that `map_bus_window`'s "RAM only" caveat now has teeth.
- `capture_cpu_state` runs under `try_lock`; on contention the freeze write is
  skipped that frame (pre-existing) — freeze is best-effort per frame, which is fine.
- Don't free/resize `bus_buffers` — still append-only; unchanged.

### Test
- **Unit (`debug/mod.rs` tests):** build a `DebugState`, `install_bus_window`
  (Work RAM), call `write_addr(base+0x100, 4, 0xDEADBEEF)`; assert it returns
  true, `pending_bus_writes == [(base+0x100, vec![0xEF,0xBE,0xAD,0xDE])]`, and the
  snapshot Box now holds those 4 bytes. Then a non-bus `synth_region` write must
  NOT enqueue (empty `pending_bus_writes`) and must poke the host buffer directly
  (existing behavior).
- **Live (asurabld headless):** `--bus-map library/asurabld/asurabld.busmap.json`,
  `enable_writes`, `write_memory addr=0x400100 len=4 value=0xDEADBEEF`, step a
  frame, `read_memory 0x400100 4` → `EF BE AD DE` persists across several frames.
  Then `freeze` a Work-RAM value and confirm it holds while the game runs.

---

## Change 2 — P2 input injection

### Current single-port wiring
- `CallbackContext.input_state: [bool;12]` — `frontend.rs:790`; init `:813`.
- `input_state_callback` hardcodes port 0 — `frontend.rs:1014-1020`.
- `Frontend::set_input` sets the single array — `frontend.rs:395-397`.
- `DebugState.injected_input: [u16;12]` — `debug/mod.rs:531`; init `:655`;
  `take_injected_input` — `debug/mod.rs:940-949`.
- MCP `press_buttons` writes port 0 only — `server.rs:917-958` (`ds.injected_input[i]=frames`,
  `"port": 0` in output); dispatch `server.rs:2079-2100`; schema `server.rs:1449-1459`.
- GUI `read_input` — `main.rs:307-338`: one keymap `:316-329`, folds
  `take_injected_input` `:332-337`, `emu.0.set_input(bits)` `:338`.
- Headless fold — `main.rs:220-227`.

The FFI already carries `port`: `RetroInputStateFn(port,device,index,id)`
(`libretro.rs:174`); `static_input_state_callback` forwards it unchanged
(`frontend.rs:1052-1056`). **No signature change needed** — only a `port==1` arm.
The core polls each port after `input_poll`: for each button `id` it calls
`input_state(port, RETRO_DEVICE_JOYPAD, 0, id)`. fbalpha2012 fighting games poll
port 1 automatically; asurabld reads one gamepad word `$810000` and splits it by
nibble (low = P1, high = P2 — `asurabld.md` §input-service), so port 1 MUST be
driven for the AI/human-2 slot to move.

### The delta (~40 lines)
Use a **parallel second array** (not an array-of-2) to minimize churn and keep
the existing `injected_input` unit tests (`server.rs:2618-2641`) intact.

**(2a) `frontend.rs` CallbackContext:**
- Add `pub input_state2: [bool; 12]` next to `input_state` (`:790`); init
  `input_state2: [false; 12]` (`:813`).
- Add `pub fn set_input2(&mut self, state: [bool;12]) { self.callback_context.input_state2 = state; }`
  next to `set_input` (`:395`).
- Rewrite `input_state_callback` (`:1014-1020`):
```rust
fn input_state_callback(&self, port: u32, device: u32, _index: u32, id: u32) -> i16 {
    if device == RETRO_DEVICE_JOYPAD && (id as usize) < 12 {
        match port {
            0 => self.input_state[id as usize] as i16,
            1 => self.input_state2[id as usize] as i16,
            _ => 0,
        }
    } else { 0 }
}
```

**(2b) `debug/mod.rs`:**
- Add `pub injected_input2: [u16; 12]` next to `injected_input` (`:531`); init
  `:655`.
- Add `take_injected_input2` mirroring `take_injected_input` (`:940-949`) but on
  `injected_input2`. (Or factor a private `take_injected(port)` — either way keep
  the existing public `take_injected_input` name so callers/tests don't break.)

**(2c) `server.rs` press_buttons:**
- Add a `port: usize` param to `press_buttons` (`:917`); select the target array:
  `let arr = if port==1 { &mut ds.injected_input2 } else { &mut ds.injected_input };`
  and set `"port": port` in the JSON. Validate `port ∈ {0,1}` (reject others).
- Dispatch (`:2079-2100`): read `let port = get_u("port").unwrap_or(0) as usize;`
  pass it through.
- Schema (`:1449-1459`): add `"port": { "type":"integer", "description":"Controller
  port 0 (P1, default) or 1 (P2)" }`; keep only `buttons` required.
- Update the tool description (`:1624-1631`) to mention port.

**(2d) `main.rs` GUI `read_input` (`:307-360`):** add a P2 keymap and a second
fold + `set_input2`. Suggested P2 keys (no overlap with P1's Z/X/A/S/Q/W/arrows/
Shift/Enter): numpad or `IJKL`+`UO`+`TG`+`YH`. Build `bits2`, OR
`take_injected_input2`, then `emu.0.set_input2(bits2)`. Keep P1 exactly as-is.

**(2e) `main.rs` headless (`:220-227`):** after the P1 fold, add:
```rust
let injected2 = match debug_state.lock() { Ok(mut ds) => ds.take_injected_input2(), Err(_) => [false;12] };
frontend.set_input2(injected2);
```
Headless has no keyboard, so P2 is injection-only there.

### Risks / gotchas
- Confirm the loaded core actually polls port 1 (fighting games do; a 1P-only core
  simply never calls the `port==1` arm — harmless).
- Keep `injected_input` (P1) as the existing array so `server.rs:2618-2641` tests
  pass unchanged; only ADD the P2 array/fn.
- `DebugState.input_state`/`input_history` stay P1-only in this change; a P2
  history field belongs to change 3 (see below), not here.

### Test
- **Live (asurabld):** driven controllable round; `press_buttons port=1
  buttons=["right"]`, then read the P2 actor X at `$405300 + 0x54` (P2 mirror of
  P1's `+0x54`, stride `0x0DB4` — `asurabld.md` §actor-p2) and confirm it ramps
  while P1's `$40454C+0x54` does not.
- **Unit:** `injected_input2` decrements independently of `injected_input`
  (mirror `take_injected_input_holds_then_releases`, `server.rs:2634`).

---

## Change 3 — per-frame recorder

### What already exists
- `run_frame` pushes `ds.push_input(self.callback_context.input_state, self.frame_count)`
  (`frontend.rs:646`) into `input_history: VecDeque<(u64,[bool;12])>` cap 120
  (`debug/mod.rs:515`, `push_input` `:928-934`).
- Actor bytes are ALREADY in memory after the frame: the Work RAM bus window
  (`$400000 len 0x10000`, `asurabld.busmap.json`) covers both actor structs
  (P1 `$40454C-$4052FF`, P2 `$405300-$4060B3`; `asurabld.md` §actor-p1/§actor-p2),
  and `refresh_bus_windows(None)` (`frontend.rs:699`) fills its Box each frame.
  So the recorder reads them from the live snapshot via `ds.read_addr` /
  region slice — **no extra core read needed**, as long as a Work RAM window is
  mapped (it is, via the busmap). If the actor range isn't covered by a window,
  fall back to `core.sek_read_block(addr,len)` on the emu thread.
- Atomic sidecar idiom to copy: `save_regions_sidecar` / `save_busmap_sidecar`
  (`frontend.rs:1113-1190`) — `.tmp` write + `rename`.

### The delta

**(3a) CLI flag — `main.rs` `Args` (`:30-53`).** Add after `bus_map` (`:52`):
```rust
/// Record a per-frame trace (actor structs + P1/P2 input + controllable flag)
/// to this JSON path. Format defined by Wave 0A's shadow/SPEC.md.
#[arg(long, value_name = "PATH")] record: Option<PathBuf>,
```

**(3b) Recorder type.** New small module `src/record.rs` (add `mod record;` to
`main.rs:1-9`) or a struct in `frontend.rs`. Prefer a module — it keeps
`frontend.rs` (already the 3-way overlap hot spot) smaller.
```rust
#[derive(serde::Serialize)]
pub struct FrameRecord {
    pub frame: u64,
    pub p1_input: [bool;12],
    pub p2_input: [bool;12],
    pub controllable: bool,
    // actor struct raw bytes (hex or base64 per SPEC.md). Kept as Vec<u8> +
    // serde_bytes, or hex strings — match SPEC.md exactly.
    pub actor1: Vec<u8>,
    pub actor2: Vec<u8>,
}
pub struct FrameRecorder {
    path: std::path::PathBuf,
    a1: (u32, usize),   // actor1 (addr,len), from SPEC.md / busmap
    a2: (u32, usize),   // actor2 (addr,len)
    buf: Vec<FrameRecord>,
    flush_every: usize, // e.g. 300 frames
}
```
Actor `(addr,len)` come from SPEC.md (default asurabld: `(0x40454C, 0x0DB4)` and
`(0x405300, 0x0DB4)`). **DEPENDENCY: Wave 0A's `shadow/SPEC.md` does not exist yet
(confirmed — `find` for SPEC*.md returns nothing).** Treat the record schema,
field names, on-disk format (single JSON array vs JSONL), and the `controllable`
source as SPEC-owned; the struct above is the working assumption to align with it.

**(3c) `Frontend` field + hook.**
- Add `recorder: Option<record::FrameRecorder>` to `Frontend` (`frontend.rs:11-28`),
  init `None` in `new` (`:75-86`). Add a setter
  `pub fn set_recorder(&mut self, path: PathBuf)` (avoids widening the already-long
  `Frontend::new` signature; both GUI and headless call it after construction).
- In `run_frame`, after `refresh_bus_windows(None)` (`:699`) and after
  `drain_bus_writes` from change 1, add `self.record_frame();`.
- `record_frame(&mut self)`:
  - return early if `self.recorder` is None;
  - read `p1 = self.callback_context.input_state`, `p2 = input_state2` (change 2);
    (recording the callback context is correct — main.rs folds keyboard+injection
    into it BEFORE `run_frame`, so it is the actual input for this frame);
  - lock `debug_state`, slice actor bytes from the Work RAM snapshot
    (`read_addr` byte-by-byte, or a `bus_buffers`/region slice helper), read the
    `controllable` bool (SPEC source — e.g. a `DebugState` flag set by the
    fight-detector / a Lua predicate; see Risks);
  - push a `FrameRecord`; if `buf.len() % flush_every == 0`, flush.
- **Flush** (`FrameRecorder::flush`): serialize `buf` to `<path>.tmp`, `rename`
  over `<path>` — the same atomic idiom as `save_busmap_sidecar`
  (`frontend.rs:1180-1189`). Final flush in `Frontend::shutdown` (`:764-771`),
  before `unload_game`.

**(3d) Wire the setter — `main.rs`.** In both paths, if `args.record.is_some()`
call `frontend.set_recorder(path)`: for GUI before building the `App`
(around `:112-118`), for headless before the loop (`run_headless`, ~`:205`).
`run_headless` already receives `args` (`:204`), so it can read `args.record`.

### Flush strategy
Primary: **buffer + periodic atomic full rewrite** (`.tmp` + rename), matching the
repo idiom and giving a always-valid-JSON file. A match is bounded, but note the
memory/rewrite cost: ~7 KB/actor-pair × 2 × frames; a multi-minute session at
60fps is tens of MB and O(n) rewrite per flush. If SPEC.md wants long recordings,
switch to **JSONL append** (open with append, write only the buffered batch, clear
buf) — bounded memory, crash loses only the un-flushed tail. Pick per SPEC.md.

### Risks / gotchas
- **`shadow/SPEC.md` missing** — the record format, field encoding, actor
  coordinates, and the `controllable` definition are all SPEC-owned. Build against
  the assumed schema above and reconcile when SPEC lands. This is change 3's
  biggest unknown.
- `controllable` needs a source. Candidates from the RE notes: the green-bar fight
  detector (project memory: fighting-game-re-methodology) or a game flag (asurabld
  `$40646E` round-over latch / fight-loop active, `asurabld.md` §Hop flags). Likely
  a `DebugState.controllable: bool` set each frame by a Lua overlay or a native
  predicate — coordinate with SPEC. The recorder just serializes the bool.
- Actor bytes require the Work RAM window mapped; assert at `set_recorder` time and
  log a warning if the covering window is absent (else fall back to a direct
  `sek_read_block`).
- Keep `record_frame` off the paused path is unnecessary — `run_frame` returns
  early when paused (`:687`) before reaching the hook, so paused frames aren't
  recorded (correct).

### Test
- **Integration (headless):** `--record /tmp/rec.json --bus-map …asurabld…`, drive
  a few seconds with `press_buttons` (P1 and P2), stop; assert the file is valid
  JSON, frames strictly increasing, `actor1.len()==0xDB4`, `actor2.len()==0xDB4`,
  and `p1_input`/`p2_input` reflect the driven buttons on the expected frames.
- **Unit:** `FrameRecorder` with tiny actor lens, append K records, `flush` to a
  temp path, re-read and parse → K records with the right frame numbers.

---

## File-ownership matrix

| File | Change 1 (Sek write) | Change 2 (P2 input) | Change 3 (recorder) |
|---|---|---|---|
| `src/libretro.rs` | **sek_write_block** (~+30, after :543); use SekWrite typedefs :696 | — | — |
| `src/debug/mod.rs` | `pending_bus_writes` field (:468) + init (:627); `write_addr`→`&mut`+routing (:811) | `injected_input2` field (:531)+init(:655); `take_injected_input2` (:940) | (reads via `read_addr`; maybe `controllable` flag field) |
| `src/frontend.rs` | `drain_bus_writes` (new); **run_frame ordering :696-699** | `CallbackContext.input_state2` (:790/:813); `set_input2` (:395); `input_state_callback` arm (:1014) | `recorder` field (:11-28/:75); `set_recorder`; `record_frame` in **run_frame :699**; `shutdown` flush (:764) |
| `src/mcp/server.rs` | — (write_memory/freeze unchanged; now functional) | `press_buttons` port param (:917); dispatch (:2079); schema (:1449); desc (:1624) | — |
| `src/main.rs` | — | `read_input` P2 keymap+set_input2 (:307-360); headless fold (:220-227) | `Args.record` (:52); `set_recorder` wiring (GUI ~:112, headless ~:205); `mod record` |
| `src/record.rs` | — | — | **new module** (FrameRecorder/FrameRecord) |

### Overlap hot spots (the real conflict surface)
1. **`frontend.rs` `run_frame` tail (:696-699)** — change 1 inserts
   `drain_bus_writes()`, change 3 inserts `record_frame()`, both immediately
   around `refresh_bus_windows`. Required order:
   `capture_cpu_state → drain_bus_writes → refresh_bus_windows → record_frame`.
2. **`frontend.rs` `CallbackContext` struct + init (:784-822)** — change 2 adds
   `input_state2`, change 3 reads it. Change 3 depends on change 2.
3. **`debug/mod.rs` `DebugState` struct + `new()` (:439-681)** — change 1 adds
   `pending_bus_writes`, change 2 adds `injected_input2` (+ possibly change 3's
   `controllable`). Adjacent-line additions.
4. **`main.rs` `Args` + `main()`/`run_headless`** — change 2 (input folds) and
   change 3 (record flag/wiring) both edit the input area and arg struct.

### Recommended Wave-1 split

**Default: ONE cohesive "engine" agent does 1 → 2 → 3 serially in a single
worktree.** Rationale: the three changes converge on the same four hot spots
above; change 3 *depends* on change 2 (needs `input_state2` for P2 records) and
benefits from change 1 (freeze/write actually working). A single agent gets the
`run_frame` ordering right in one pass and produces zero internal merge conflicts.
Coupling here outweighs parallelism.

**If parallelism is required, split 2-ways, not 3:**
- **Agent X = Change 1** — almost fully isolated: owns `libretro.rs` entirely, owns
  `write_addr` in `debug/mod.rs`, adds `drain_bus_writes` + one line in `run_frame`.
  Touches no input/recorder code.
- **Agent Y = Changes 2 + 3 together** — they share `CallbackContext`, `main.rs`
  input wiring, and 3 needs 2's `input_state2`; keeping them in one agent removes
  their mutual conflict and their dependency ordering.

Only cross-agent overlap is then trivial adjacent-line merges: the `DebugState`
struct/`new()` (X adds `pending_bus_writes`, Y adds `injected_input2`) and the
`run_frame` tail (X's `drain_bus_writes`, Y's `record_frame`). A 30-second manual
resolve. Do NOT run three parallel agents — the 3-way collisions on `run_frame`
and the two structs are not worth it.

### Cross-cutting note
`shadow/SPEC.md` (Wave 0A) is a hard input for change 3 and does not yet exist.
Change 3's agent should either wait for it or implement against the assumed schema
in §3b and reconcile. Changes 1 and 2 have no such dependency and can proceed now.
