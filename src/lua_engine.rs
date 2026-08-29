//! Minimal v1 Lua scripting layer for RustRetro.
//!
//! Exposes a tiny, sandboxed API to community scripts whose killer use case is
//! fighting-game HITBOX OVERLAYS: read big-endian object-RAM box lists and draw
//! translucent rectangles onto the framebuffer every frame.
//!
//! ## Threading
//! `mlua::Lua` and the internal `Rc<RefCell<…>>` draw buffer are `!Send`, so
//! `LuaEngine` MUST live as a Bevy `NonSend` resource on the main thread, exactly
//! like `Emu`. It is never wrapped in `Arc<Mutex<…>>`.
//!
//! ## API surface (installed into the VM)
//! ```text
//! memory.read_u8(addr)              -> integer
//! memory.read_u16_be(addr)          -> integer
//! memory.read_u32_be(addr)          -> integer
//! memory.read_s16_be(addr)          -> integer (signed)
//! memory.read_u16_le(addr)          -> integer  (little-endian)
//! memory.read_u32_le(addr)          -> integer  (little-endian)
//! memory.writebyte(addr, v)                     (GATED, see below)
//! memory.writeword(addr, v)                     (16-bit guest big-endian; GATED)
//! memory.freeze(addr, v)                        (per-frame re-write via a frozen watch; GATED)
//! memory.unfreeze(addr)                         (drop frozen watches at addr; GATED)
//! savestate.save(slot_or_path)      -> true     (queued; slot 1-9 or path string)
//! savestate.load(slot_or_path)      -> true     (queued)
//! input.set(port, mask_or_table)                (port 0/1; 2-frame hold)
//! input.get(port)                   -> integer  (12-bit mask, last folded state)
//! input.hold(port, mask_or_table)               (port 0/1; asserted every fold
//!                                                until input.release — survives
//!                                                pause, unlike input.set's countdown)
//! input.release(port [, mask_or_table])         (clear held buttons named in the
//!                                                arg, or all of them if omitted)
//! gui.drawBox(x1,y1,x2,y2, fill, line)
//! gui.drawText(x,y, str [, color [, scale]])
//! gui.text(x,y, str [, color [, scale]])        (drawText + 1px drop shadow)
//! gui.drawLine(x1,y1,x2,y2, color)
//! gui.drawPixel(x,y, color)
//! event.onframeend(function)
//! console.log(str)
//! emu.framecount()                  -> integer
//! emu.paused()                      -> bool
//! game.controllable()               -> bool     (profile gate vs live memory)
//! game.addr(name)                   -> integer|nil  (named global from the profile)
//! game.block1() / game.block2()     -> integer  (fighter block base addresses)
//! game.field_off(name)              -> integer|nil  (fighter-field offset)
//! game.char_name(id)                -> string   (roster name; "c<N>" fallback)
//! game.matchup_slug(me, opp)        -> string   ("goat-vs-rosemary")
//! game.stage_value_for(opp)         -> integer|nil  (stage-selector value)
//! game.calibration(key)             -> number|nil
//! training.enabled()                -> bool     (native training mode on?)
//! training.refill()                 -> bool     (native health refill on?)
//! training.dummy()                  -> string   ("free"/"stand"/"crouch"/"jump"/"block"/"block_punish")
//! training.punish_state()          -> string   (BlockPunish phase, same string the
//!                                                panel shows: "guarding — ARMED" /
//!                                                "cooling — Nf" / "punishing: slide")
//! training.guard_mode()             -> string   ("all"/"after_first_hit"/"random"/"none")
//! training.set_enabled(bool)                    (write-gated; on = refill on, F5 parity)
//! training.set_dummy(mode)                      (write-gated; headless F1 — same mode strings)
//! training.set_guard(mode [, pct])               (write-gated; guard mode + Random's percent)
//! training.set_punish(pool)                     (write-gated; BlockPunish pool:
//!                                                {{weight=3, move="slide"}, {weight=2, attack="HP"},
//!                                                 {weight=1, continue_frames=30}, ...})
//! shadow.on()                       -> bool|nil (nil = no model loaded)
//! shadow.model()                    -> string|nil  (loaded model name)
//! shadow.toggle()                                (queue a shadow on/off toggle)
//! record.active()                   -> bool
//! record.path()                     -> string|nil
//! record.frames()                   -> integer
//! record.start(path [, style])      -> true     (queued; errors if already recording)
//! record.stop()                     -> true     (queued)
//! _RUSTRETRO_API                    = 3  (version sentinel)
//! ```
//! Colors are packed RGBA u32: `0xRRGGBBAA`.
//!
//! ## API v3: the profile boundary
//! v3 adds the `game`/`training`/`shadow`/`record` tables plus `emu.paused`
//! and `memory.freeze`/`unfreeze`. The design ruling (docs/game-profiles.md):
//! **logic lives once, in Rust; Lua ASKS via bindings.** Scripts never carry
//! raw addresses (`game.addr("round_timer")`, not `0x40000A`) and never
//! re-implement the controllable gate or the enforcement trio — native
//! training mode owns those; `game.controllable()` evaluates the loaded
//! profile's gate condition list against live memory with the same semantics
//! as `record.rs`'s recorder gate. (`record` was checked against every global
//! this engine installs — no collision, so it keeps the natural name.)
//!
//! ## FBNeo/FBA naming
//! The write/savestate/input/gui.text names follow the FBNeo Lua conventions
//! (`memory.writebyte`, `savestate.save`, `joypad`-style button tables,
//! `gui.text`) because that ecosystem's scripts are our porting reference.
//!
//! ## Write gate
//! `memory.writebyte`/`memory.writeword` are refused with a Lua error unless
//! [`DebugState::lua_writes_enabled`] is on — armed by launching with
//! `--training`, or at runtime by the MCP `enable_writes` tool (and re-locked
//! by `disable_writes`). `savestate.load` is deliberately NOT behind that gate:
//! scripts are user-authored and loading a state can't corrupt live RAM in a
//! way the user didn't ask for — it's the same trust level as the F-key /
//! `--load-state` paths, whereas raw pokes can wedge the emulated machine.
//!
//! ## Endianness of `memory.writeword`
//! Guest (68k) order, i.e. big-endian: the HIGH byte lands at `addr`, matching
//! how `memory.read_u16_be` reads. `writeword(a, v)` followed by
//! `read_u16_be(a)` round-trips `v & 0xFFFF`.

use std::cell::RefCell;
use std::rc::Rc;

use mlua::{Lua, LuaOptions, RegistryKey, StdLib};

use crate::debug::SharedDebugState;

/// A single frame-local drawing command pushed by a script via the `gui` table.
#[derive(Clone, Debug)]
pub enum DrawCmd {
    /// Filled + outlined rectangle. `fill`/`line` are packed `0xRRGGBBAA`.
    Box {
        x1: i32,
        y1: i32,
        x2: i32,
        y2: i32,
        fill: u32,
        line: u32,
    },
    /// Text label. `color` is packed `0xRRGGBBAA`. `scale` magnifies the 3×5
    /// bitmap font (each glyph pixel becomes a `scale`×`scale` block); 1 = native.
    /// `shadow` adds a 1px black drop shadow (down-right, same alpha as `color`)
    /// for legibility over bright game art — used by `gui.text`.
    Text {
        x: i32,
        y: i32,
        s: String,
        color: u32,
        scale: i32,
        shadow: bool,
    },
    /// A straight line from (x1,y1) to (x2,y2). `color` is packed `0xRRGGBBAA`.
    Line {
        x1: i32,
        y1: i32,
        x2: i32,
        y2: i32,
        color: u32,
    },
    /// A single pixel at (x,y). `color` is packed `0xRRGGBBAA`.
    Pixel {
        x: i32,
        y: i32,
        color: u32,
    },
}

/// Shared, single-threaded draw buffer. `Rc<RefCell<…>>` is fine because the VM
/// and all its closures run only on the main thread.
type DrawBuf = Rc<RefCell<Vec<DrawCmd>>>;

/// Lock the shared debug state briefly and read one byte. Out-of-map reads
/// return 0 (mirrors how community emulator scripts behave). The `MutexGuard`
/// is dropped before this returns, keeping the borrow out of the Lua closure.
fn read1(dbg: &SharedDebugState, addr: u32) -> mlua::Result<u8> {
    let ds = dbg.lock().map_err(|e| mlua::Error::external(e.to_string()))?;
    let b = ds.read_u8(addr).unwrap_or(0);
    drop(ds);
    Ok(b)
}

/// Shared body of the Lua memory-write bindings. Refuses (Lua error naming the
/// gate) unless [`DebugState::lua_writes_enabled`] is on, then routes through
/// `DebugState::write_addr` — which pokes the snapshot AND, for bus-window
/// regions, queues the real poke for the live 68k bus. `le_value` carries the
/// value bytes in ascending-address order exactly as `write_addr` expects; the
/// `writeword` caller pre-swaps so the guest sees big-endian.
fn write_guest(dbg: &SharedDebugState, addr: u32, len: usize, le_value: u32) -> mlua::Result<()> {
    let mut ds = dbg.lock().map_err(|e| mlua::Error::external(e.to_string()))?;
    if !ds.lua_writes_enabled {
        return Err(writes_gate_error());
    }
    if !ds.write_addr(addr as usize, len, le_value) {
        return Err(mlua::Error::RuntimeError(format!(
            "memory write failed: no writable region contains 0x{addr:X}"
        )));
    }
    Ok(())
}

/// The Lua error every gated mutation binding raises while
/// [`DebugState::lua_writes_enabled`] is off. One shared constructor so
/// `memory.writebyte`/`writeword`/`freeze`/`unfreeze` all name the gate the
/// same way (scripts and tests match on the substring "lua_writes_enabled").
fn writes_gate_error() -> mlua::Error {
    mlua::Error::RuntimeError(
        "memory write blocked: the lua_writes_enabled gate is OFF \
         (launch with --training or arm it via the MCP enable_writes tool)"
            .to_string(),
    )
}

/// The controllable-gate evaluator lives in `gate::eval_gate` — ONE gate
/// shared by Lua's `game.controllable()`, training enforcement, and (locked
/// by a unit test below) the recorder's composite.
use crate::gate::eval_gate;

/// Frames `input.set` holds each pressed button — the same 2-frame idiom as the
/// training-mode dummy injection (`training::tick`): long enough to bridge into
/// the next input fold, short enough that a per-frame callback re-asserting a
/// button reads as continuously held and a dropped button releases in ≤2 frames.
const INPUT_SET_HOLD_FRAMES: u16 = 2;

/// Decode a 12-bit integer button mask (bit i = `RETRO_DEVICE_ID_JOYPAD` id i:
/// 0=B 1=Y 2=Select 3=Start 4=Up 5=Down 6=Left 7=Right 8=A 9=X 10=L 11=R).
fn buttons_from_mask(mask: i64) -> Result<[bool; 12], String> {
    if !(0..=0xFFF).contains(&mask) {
        return Err(format!(
            "input: mask must be 0..=0xFFF (12 buttons), got {mask}"
        ));
    }
    let mut bits = [false; 12];
    for (i, b) in bits.iter_mut().enumerate() {
        *b = (mask >> i) & 1 == 1;
    }
    Ok(bits)
}

/// Decode `{up=true, b=true, ...}`-style name/pressed pairs (RETRO button names,
/// shared with the MCP `press_buttons` tool). Unknown names are an error so a
/// typo ("bb") fails loudly instead of silently doing nothing.
fn buttons_from_pairs(pairs: &[(String, bool)]) -> Result<[bool; 12], String> {
    let mut bits = [false; 12];
    for (name, pressed) in pairs {
        let Some(i) = crate::mcp::server::joypad_button_index(name) else {
            return Err(format!(
                "input: unknown button '{name}' (valid: b y select start up down left right a x l r)"
            ));
        };
        bits[i] = *pressed;
    }
    Ok(bits)
}

/// Decode an `input.set`/`input.hold`/`input.release` second argument — an
/// integer mask or a `{name=bool, ...}` table — into a 12-button bitmap. Shared
/// so `input.hold`'s table/mask acceptance matches `input.set` exactly.
fn decode_button_spec(spec: &mlua::Value) -> Result<[bool; 12], String> {
    match spec {
        mlua::Value::Integer(m) => buttons_from_mask(*m),
        mlua::Value::Number(f) if f.fract() == 0.0 => buttons_from_mask(*f as i64),
        mlua::Value::Table(t) => {
            let mut pairs = Vec::new();
            for kv in t.clone().pairs::<String, mlua::Value>() {
                let (k, v) = kv.map_err(|e| {
                    format!("input: button table keys must be strings: {e}")
                })?;
                // Lua truthiness: only false/nil mean released.
                let pressed = !matches!(v, mlua::Value::Nil | mlua::Value::Boolean(false));
                pairs.push((k, pressed));
            }
            buttons_from_pairs(&pairs)
        }
        other => Err(format!(
            "input: expected an integer mask or a button table, got {}",
            other.type_name()
        )),
    }
}

/// Pack a 12-button state into the `input.get` mask (bit i = button i pressed).
fn mask_from_buttons(bits: &[bool; 12]) -> u32 {
    bits.iter()
        .enumerate()
        .fold(0u32, |m, (i, b)| m | ((*b as u32) << i))
}

/// Parse the `savestate.save`/`savestate.load` argument: an integer 1-9 is a
/// slot (resolved to `<save_dir>/<rom_stem>.stateN` by the Frontend), a string
/// is a filesystem path. Anything else is an error.
fn parse_state_target(v: &mlua::Value, load: bool) -> Result<crate::debug::StateOp, String> {
    use crate::debug::StateOp;
    let slot_op = |n: i64| -> Result<StateOp, String> {
        if !(1..=9).contains(&n) {
            return Err(format!("savestate: slot must be 1-9, got {n}"));
        }
        Ok(if load {
            StateOp::LoadSlot(n as u8)
        } else {
            StateOp::SaveSlot(n as u8)
        })
    };
    match v {
        mlua::Value::Integer(n) => slot_op(*n),
        mlua::Value::Number(f) if f.fract() == 0.0 => slot_op(*f as i64),
        mlua::Value::String(s) => {
            let p = s.to_string_lossy().trim().to_string();
            if p.is_empty() {
                return Err("savestate: path must be a non-empty string".to_string());
            }
            let pb = std::path::PathBuf::from(p);
            Ok(if load { StateOp::Load(pb) } else { StateOp::Save(pb) })
        }
        other => Err(format!(
            "savestate: expected a slot number (1-9) or a path string, got {}",
            other.type_name()
        )),
    }
}

/// The Lua scripting engine. Owns the VM, the registered `event.onframeend`
/// callbacks, and the frame-local draw-command buffer.
pub struct LuaEngine {
    lua: Lua,
    /// Registered `event.onframeend` callbacks (in registration order).
    frame_callbacks: Rc<RefCell<Vec<RegistryKey>>>,
    /// Frame-local draw commands produced by `gui.*` during callback execution.
    draw_cmds: DrawBuf,
    /// Shared debug state, kept so error reporting can log to the event log.
    debug: SharedDebugState,
}

impl LuaEngine {
    /// Create a sandboxed VM and install the API tables.
    ///
    /// Sandbox: only base/table/string/math stdlibs are loaded. `io`, `os`, and
    /// `package` are deliberately excluded so scripts cannot touch the filesystem,
    /// spawn processes, or load native modules. The HOST reads script files; Lua
    /// never does.
    pub fn new(debug: SharedDebugState) -> mlua::Result<Self> {
        // Restricted stdlib set — no io/os/package/debug.
        let libs = StdLib::TABLE | StdLib::STRING | StdLib::MATH;
        let lua = Lua::new_with(libs, LuaOptions::default())?;

        let draw_cmds: DrawBuf = Rc::new(RefCell::new(Vec::new()));
        let frame_callbacks: Rc<RefCell<Vec<RegistryKey>>> = Rc::new(RefCell::new(Vec::new()));

        let engine = LuaEngine {
            lua,
            frame_callbacks: Rc::clone(&frame_callbacks),
            draw_cmds: Rc::clone(&draw_cmds),
            debug: SharedDebugState::clone(&debug),
        };

        engine.install_api(&debug, &draw_cmds, &frame_callbacks)?;
        Ok(engine)
    }

    /// Install the `memory`, `gui`, `event`, and `console` global tables.
    fn install_api(
        &self,
        debug: &SharedDebugState,
        draw_cmds: &DrawBuf,
        frame_callbacks: &Rc<RefCell<Vec<RegistryKey>>>,
    ) -> mlua::Result<()> {
        let lua = &self.lua;
        let globals = lua.globals();

        // ── memory.* ──────────────────────────────────────────────────────────
        let memory = lua.create_table()?;

        // read_u8(addr) -> integer
        {
            let dbg = SharedDebugState::clone(debug);
            let f = lua.create_function(move |_, addr: u32| -> mlua::Result<u32> {
                let b = read1(&dbg, addr)?;
                Ok(b as u32)
            })?;
            memory.set("read_u8", f)?;
        }

        // read_u16_be(addr) -> integer  (big-endian: byte[addr] is high byte)
        {
            let dbg = SharedDebugState::clone(debug);
            let f = lua.create_function(move |_, addr: u32| -> mlua::Result<u32> {
                let hi = read1(&dbg, addr)? as u32;
                let lo = read1(&dbg, addr.wrapping_add(1))? as u32;
                Ok((hi << 8) | lo)
            })?;
            memory.set("read_u16_be", f)?;
        }

        // read_u32_be(addr) -> integer
        {
            let dbg = SharedDebugState::clone(debug);
            let f = lua.create_function(move |_, addr: u32| -> mlua::Result<u32> {
                let b0 = read1(&dbg, addr)? as u32;
                let b1 = read1(&dbg, addr.wrapping_add(1))? as u32;
                let b2 = read1(&dbg, addr.wrapping_add(2))? as u32;
                let b3 = read1(&dbg, addr.wrapping_add(3))? as u32;
                Ok((b0 << 24) | (b1 << 16) | (b2 << 8) | b3)
            })?;
            memory.set("read_u32_be", f)?;
        }

        // read_s16_be(addr) -> integer  (sign-extended 16-bit big-endian)
        {
            let dbg = SharedDebugState::clone(debug);
            let f = lua.create_function(move |_, addr: u32| -> mlua::Result<i32> {
                let hi = read1(&dbg, addr)? as u16;
                let lo = read1(&dbg, addr.wrapping_add(1))? as u16;
                let raw = (hi << 8) | lo;
                Ok(raw as i16 as i32)
            })?;
            memory.set("read_s16_be", f)?;
        }

        // read_u16_le(addr) -> integer  (little-endian: byte[addr] is low byte)
        {
            let dbg = SharedDebugState::clone(debug);
            let f = lua.create_function(move |_, addr: u32| -> mlua::Result<u32> {
                let lo = read1(&dbg, addr)? as u32;
                let hi = read1(&dbg, addr.wrapping_add(1))? as u32;
                Ok((hi << 8) | lo)
            })?;
            memory.set("read_u16_le", f)?;
        }

        // read_u32_le(addr) -> integer  (little-endian)
        {
            let dbg = SharedDebugState::clone(debug);
            let f = lua.create_function(move |_, addr: u32| -> mlua::Result<u32> {
                let b0 = read1(&dbg, addr)? as u32;
                let b1 = read1(&dbg, addr.wrapping_add(1))? as u32;
                let b2 = read1(&dbg, addr.wrapping_add(2))? as u32;
                let b3 = read1(&dbg, addr.wrapping_add(3))? as u32;
                Ok(b0 | (b1 << 8) | (b2 << 16) | (b3 << 24))
            })?;
            memory.set("read_u32_le", f)?;
        }

        // writebyte(addr, v) — FBNeo-style name. GATED on lua_writes_enabled.
        {
            let dbg = SharedDebugState::clone(debug);
            let f = lua.create_function(move |_, (addr, v): (u32, u32)| {
                write_guest(&dbg, addr, 1, v & 0xFF)
            })?;
            memory.set("writebyte", f)?;
        }

        // writeword(addr, v) — 16-bit in GUEST (68k big-endian) order: high byte
        // at addr, mirroring read_u16_be. write_addr wants ascending-address
        // (little-endian) value bytes, so swap — same idiom as training::wr16be.
        {
            let dbg = SharedDebugState::clone(debug);
            let f = lua.create_function(move |_, (addr, v): (u32, u32)| {
                write_guest(&dbg, addr, 2, (v as u16).swap_bytes() as u32)
            })?;
            memory.set("writeword", f)?;
        }

        // freeze(addr, v) — pin a byte by installing a FROZEN watch (the emu
        // thread re-writes frozen watches every frame, exactly the mechanism
        // the Watch panel's freeze checkbox and the matchup panel's stage
        // force use). GATED like writebyte: a freeze is a standing write.
        // Replaces any existing frozen watch at the same address.
        {
            let dbg = SharedDebugState::clone(debug);
            let f = lua.create_function(move |_, (addr, v): (u32, u32)| {
                let mut ds = dbg.lock().map_err(|e| mlua::Error::external(e.to_string()))?;
                if !ds.lua_writes_enabled {
                    return Err(writes_gate_error());
                }
                ds.watches
                    .retain(|w| w.addr != addr as usize || !w.frozen);
                ds.watches.push(crate::debug::Watch {
                    addr: addr as usize,
                    label: "lua freeze".to_string(),
                    format: crate::debug::WatchFormat::Hex8,
                    frozen: true,
                    frozen_value: Some(v & 0xFF),
                    track_changes: false,
                    current: None,
                    prev_value: None,
                });
                Ok(())
            })?;
            memory.set("freeze", f)?;
        }

        // unfreeze(addr) — drop any frozen watch at addr (non-frozen watches
        // there survive). GATED the same way: it mutates the standing-write
        // set, so it is part of the same opt-in surface.
        {
            let dbg = SharedDebugState::clone(debug);
            let f = lua.create_function(move |_, addr: u32| {
                let mut ds = dbg.lock().map_err(|e| mlua::Error::external(e.to_string()))?;
                if !ds.lua_writes_enabled {
                    return Err(writes_gate_error());
                }
                ds.watches
                    .retain(|w| w.addr != addr as usize || !w.frozen);
                Ok(())
            })?;
            memory.set("unfreeze", f)?;
        }

        globals.set("memory", memory)?;

        // ── savestate.* ───────────────────────────────────────────────────────
        // Queued, not immediate: core FFI may only happen on the emu thread, so
        // save/load set `pending_state_op` and Frontend::drain_state_op applies
        // it on the next frame. Returns true when enqueued; raises a Lua error
        // when another op is still in flight. NOT behind the write gate (see
        // module docs — user-authored scripts get the same trust as hotkeys).
        let savestate = lua.create_table()?;
        for (name, load) in [("save", false), ("load", true)] {
            let dbg = SharedDebugState::clone(debug);
            let f = lua.create_function(move |_, v: mlua::Value| -> mlua::Result<bool> {
                let op = parse_state_target(&v, load).map_err(mlua::Error::RuntimeError)?;
                let mut ds = dbg.lock().map_err(|e| mlua::Error::external(e.to_string()))?;
                if ds.pending_state_op.is_some() {
                    return Err(mlua::Error::RuntimeError(
                        "savestate: another save/load is already queued (ops apply on the \
                         next frame drain) — retry on a later frame"
                            .to_string(),
                    ));
                }
                ds.pending_state_op = Some(op);
                Ok(true)
            })?;
            savestate.set(name, f)?;
        }
        globals.set("savestate", savestate)?;

        // ── input.* ───────────────────────────────────────────────────────────
        let input = lua.create_table()?;

        // set(port, mask_or_table) — port 0 (P1) / 1 (P2). Accepts a 12-bit
        // integer mask (bit i = RETRO id i) or {up=true, b=true, ...} with RETRO
        // names. Overwrites ALL 12 hold counters: pressed → 2-frame hold,
        // absent/false → released (same idiom as the training dummy), so a
        // per-frame onframeend callback holding a button "just works".
        {
            let dbg = SharedDebugState::clone(debug);
            let f = lua.create_function(move |_, (port, spec): (u32, mlua::Value)| {
                if port > 1 {
                    return Err(mlua::Error::RuntimeError(
                        "input.set: port must be 0 (P1) or 1 (P2)".to_string(),
                    ));
                }
                let bits = decode_button_spec(&spec)
                    .map_err(|e| mlua::Error::RuntimeError(e.replace("input:", "input.set:")))?;
                let mut ds = dbg.lock().map_err(|e| mlua::Error::external(e.to_string()))?;
                let arr = if port == 1 {
                    &mut ds.injected_input2
                } else {
                    &mut ds.injected_input
                };
                for (i, on) in bits.iter().enumerate() {
                    arr[i] = if *on { INPUT_SET_HOLD_FRAMES } else { 0 };
                }
                Ok(())
            })?;
            input.set("set", f)?;
        }

        // get(port) -> integer mask of the port's CURRENT buttons (the state fed
        // to the core on the last input fold — keyboard/pad OR injected). Cheap:
        // the frontend mirrors both ports into DebugState each frame.
        {
            let dbg = SharedDebugState::clone(debug);
            let f = lua.create_function(move |_, port: u32| -> mlua::Result<u32> {
                if port > 1 {
                    return Err(mlua::Error::RuntimeError(
                        "input.get: port must be 0 (P1) or 1 (P2)".to_string(),
                    ));
                }
                let ds = dbg.lock().map_err(|e| mlua::Error::external(e.to_string()))?;
                let bits = if port == 1 { ds.input_state2 } else { ds.input_state };
                Ok(mask_from_buttons(&bits))
            })?;
            input.set("get", f)?;
        }

        // hold(port, mask_or_table) — assert buttons on EVERY fold until
        // input.release clears them, independent of set's 2-frame countdown.
        // Accepts the same mask/table shapes as set. Idempotent: REPLACES the
        // port's held set (does not OR with whatever was held before), so
        // calling with a lesser set drops the buttons no longer named.
        {
            let dbg = SharedDebugState::clone(debug);
            let f = lua.create_function(move |_, (port, spec): (u32, mlua::Value)| {
                if port > 1 {
                    return Err(mlua::Error::RuntimeError(
                        "input.hold: port must be 0 (P1) or 1 (P2)".to_string(),
                    ));
                }
                let bits = decode_button_spec(&spec)
                    .map_err(|e| mlua::Error::RuntimeError(e.replace("input:", "input.hold:")))?;
                let mut ds = dbg.lock().map_err(|e| mlua::Error::external(e.to_string()))?;
                ds.set_held_input(port as usize, bits);
                Ok(())
            })?;
            input.set("hold", f)?;
        }

        // release(port [, mask_or_table]) — clear the named buttons from the
        // held set (same mask/table shapes as hold), or the WHOLE held set
        // when the second argument is omitted. Never touches an in-flight
        // input.set countdown.
        {
            let dbg = SharedDebugState::clone(debug);
            let f = lua.create_function(move |_, (port, spec): (u32, Option<mlua::Value>)| {
                if port > 1 {
                    return Err(mlua::Error::RuntimeError(
                        "input.release: port must be 0 (P1) or 1 (P2)".to_string(),
                    ));
                }
                let mut ds = dbg.lock().map_err(|e| mlua::Error::external(e.to_string()))?;
                match spec {
                    None => ds.clear_held_input(port as usize, None),
                    Some(v) => {
                        let bits = decode_button_spec(&v).map_err(|e| {
                            mlua::Error::RuntimeError(e.replace("input:", "input.release:"))
                        })?;
                        let idxs: Vec<usize> = (0..12).filter(|&i| bits[i]).collect();
                        ds.clear_held_input(port as usize, Some(&idxs));
                    }
                }
                Ok(())
            })?;
            input.set("release", f)?;
        }

        globals.set("input", input)?;

        // ── gui.* ─────────────────────────────────────────────────────────────
        let gui = lua.create_table()?;

        // drawBox(x1,y1,x2,y2, fill_rgba, line_rgba)
        {
            let buf = Rc::clone(draw_cmds);
            let f = lua.create_function(
                move |_, (x1, y1, x2, y2, fill, line): (i32, i32, i32, i32, u32, u32)| {
                    buf.borrow_mut().push(DrawCmd::Box {
                        x1,
                        y1,
                        x2,
                        y2,
                        fill,
                        line,
                    });
                    Ok(())
                },
            )?;
            gui.set("drawBox", f)?;
        }

        // drawText(x, y, str [, color [, scale]])
        {
            let buf = Rc::clone(draw_cmds);
            let f = lua.create_function(
                move |_, (x, y, s, color, scale): (i32, i32, String, Option<u32>, Option<i32>)| {
                    buf.borrow_mut().push(DrawCmd::Text {
                        x,
                        y,
                        s,
                        // Default: opaque white.
                        color: color.unwrap_or(0xFFFF_FFFF),
                        // Default native scale; clamp so a bad value can't draw nothing.
                        scale: scale.unwrap_or(1).max(1),
                        shadow: false,
                    });
                    Ok(())
                },
            )?;
            gui.set("drawText", f)?;
        }

        // text(x, y, str [, color [, scale]]) — FBNeo-style name: drawText plus
        // a 1px black drop shadow so labels stay legible over bright game art.
        {
            let buf = Rc::clone(draw_cmds);
            let f = lua.create_function(
                move |_, (x, y, s, color, scale): (i32, i32, String, Option<u32>, Option<i32>)| {
                    buf.borrow_mut().push(DrawCmd::Text {
                        x,
                        y,
                        s,
                        color: color.unwrap_or(0xFFFF_FFFF),
                        scale: scale.unwrap_or(1).max(1),
                        shadow: true,
                    });
                    Ok(())
                },
            )?;
            gui.set("text", f)?;
        }

        // drawLine(x1,y1,x2,y2, color)
        {
            let buf = Rc::clone(draw_cmds);
            let f = lua.create_function(
                move |_, (x1, y1, x2, y2, color): (i32, i32, i32, i32, u32)| {
                    buf.borrow_mut().push(DrawCmd::Line { x1, y1, x2, y2, color });
                    Ok(())
                },
            )?;
            gui.set("drawLine", f)?;
        }

        // drawPixel(x, y, color)
        {
            let buf = Rc::clone(draw_cmds);
            let f = lua.create_function(
                move |_, (x, y, color): (i32, i32, u32)| {
                    buf.borrow_mut().push(DrawCmd::Pixel { x, y, color });
                    Ok(())
                },
            )?;
            gui.set("drawPixel", f)?;
        }

        globals.set("gui", gui)?;

        // ── event.* ───────────────────────────────────────────────────────────
        let event = lua.create_table()?;

        // onframeend(function) — register a per-frame callback.
        {
            let cbs = Rc::clone(frame_callbacks);
            let f = lua.create_function(move |lua, func: mlua::Function| {
                let key = lua.create_registry_value(func)?;
                cbs.borrow_mut().push(key);
                Ok(())
            })?;
            event.set("onframeend", f)?;
        }

        globals.set("event", event)?;

        // ── console.* ─────────────────────────────────────────────────────────
        let console = lua.create_table()?;
        {
            let dbg = SharedDebugState::clone(debug);
            let f = lua.create_function(move |_, msg: String| {
                if let Ok(mut ds) = dbg.lock() {
                    ds.log(format!("[lua] {msg}"));
                }
                Ok(())
            })?;
            console.set("log", f)?;
        }
        globals.set("console", console)?;

        // ── emu.* ─────────────────────────────────────────────────────────────
        let emu = lua.create_table()?;

        // framecount() -> integer  (returns DebugState.frame_count)
        {
            let dbg = SharedDebugState::clone(debug);
            let f = lua.create_function(move |_, ()| -> mlua::Result<u64> {
                let ds = dbg.lock().map_err(|e| mlua::Error::external(e.to_string()))?;
                Ok(ds.frame_count)
            })?;
            emu.set("framecount", f)?;
        }

        // paused() -> bool  (DebugState.paused — lets a script skip per-frame
        // work, or detect frame-stepping, without polling framecount deltas)
        {
            let dbg = SharedDebugState::clone(debug);
            let f = lua.create_function(move |_, ()| -> mlua::Result<bool> {
                let ds = dbg.lock().map_err(|e| mlua::Error::external(e.to_string()))?;
                Ok(ds.paused)
            })?;
            emu.set("paused", f)?;
        }

        globals.set("emu", emu)?;

        // ── game.* ────────────────────────────────────────────────────────────
        // The profile boundary (API v3): scripts ask the loaded GameProfile for
        // addresses/names/values by NAME and ask the engine for the gate verdict
        // — they never restate per-game knowledge. `profile::current()` is safe
        // here: the profile is installed at startup before the engine exists.
        // Accessors are called at INVOCATION time, not install time, so building
        // an engine without a profile (some unit tests) stays legal as long as
        // no game.* binding runs.
        let game = lua.create_table()?;

        // controllable() -> bool — the profile's gate condition list evaluated
        // against live memory (see gate::eval_gate; same semantics as the recorder).
        {
            let dbg = SharedDebugState::clone(debug);
            let f = lua.create_function(move |_, ()| -> mlua::Result<bool> {
                let ds = dbg.lock().map_err(|e| mlua::Error::external(e.to_string()))?;
                Ok(eval_gate(&ds, crate::profile::current()))
            })?;
            game.set("controllable", f)?;
        }

        // addr(name) -> integer|nil — named global from the profile memory map.
        {
            let f = lua.create_function(|_, name: String| -> mlua::Result<Option<u32>> {
                Ok(crate::profile::current().global(&name))
            })?;
            game.set("addr", f)?;
        }

        // block1() / block2() -> integer — fighter block base addresses.
        {
            let f = lua.create_function(|_, ()| -> mlua::Result<u32> {
                Ok(crate::profile::current().block1())
            })?;
            game.set("block1", f)?;
            let f = lua.create_function(|_, ()| -> mlua::Result<u32> {
                Ok(crate::profile::current().block2())
            })?;
            game.set("block2", f)?;
        }

        // field_off(name) -> integer|nil — fighter-field OFFSET only (add to
        // block1()/block2() yourself; the size byte stays Rust-side).
        {
            let f = lua.create_function(|_, name: String| -> mlua::Result<Option<u32>> {
                Ok(crate::profile::current().field_off(&name).map(|(off, _)| off))
            })?;
            game.set("field_off", f)?;
        }

        // char_name(id) -> string; matchup_slug(me, opp) -> string.
        {
            let f = lua.create_function(|_, id: u8| -> mlua::Result<String> {
                Ok(crate::profile::current().char_name(id))
            })?;
            game.set("char_name", f)?;
            let f = lua.create_function(|_, (me, opp): (u8, u8)| -> mlua::Result<String> {
                Ok(crate::profile::current().matchup_slug(me, opp))
            })?;
            game.set("matchup_slug", f)?;
        }

        // stage_value_for(opp) -> integer|nil — selector value whose home
        // matchup is `opp` (nil when the game has no selector or no value).
        {
            let f = lua.create_function(|_, opp: u8| -> mlua::Result<Option<u8>> {
                Ok(crate::profile::current().stage_value_for_opponent(opp))
            })?;
            game.set("stage_value_for", f)?;
        }

        // calibration(key) -> number|nil — feature-scaling constants.
        {
            let f = lua.create_function(|_, key: String| -> mlua::Result<Option<f64>> {
                Ok(crate::profile::current().calibration(&key))
            })?;
            game.set("calibration", f)?;
        }

        globals.set("game", game)?;

        // ── hunt.* ────────────────────────────────────────────────────────────
        // Signal hunt (docs/signal-hunt.md §8): scripted marking from a
        // per-frame callback, for events a human cannot click fast enough —
        //   event.onframeend(function()
        //     if contact_edge() then hunt.mark("event") end
        //   end)
        // Marking is judgement, and §1 is explicit that the judgement stays the
        // human's; this binding only lets that judgement be EXPRESSED as code.
        // It is NOT behind the write gate — a mark reads memory, never pokes it.
        let hunt = lua.create_table()?;
        {
            let dbg = SharedDebugState::clone(debug);
            let f = lua.create_function(move |_, label: String| -> mlua::Result<String> {
                let ds = dbg.lock().map_err(|e| mlua::Error::external(e.to_string()))?;
                crate::hunt::mark_with(&ds, &label).map_err(mlua::Error::external)
            })?;
            hunt.set("mark", f)?;
        }
        // hunt.status() -> JSON string (region, ring fill, per-label mark
        // counts) so a script can log or overlay how the hunt is going.
        {
            let f = lua.create_function(|_, ()| -> mlua::Result<String> {
                Ok(crate::hunt::status().to_string())
            })?;
            hunt.set("status", f)?;
        }
        {
            let f = lua.create_function(|_, ()| -> mlua::Result<String> {
                Ok(crate::hunt::reset())
            })?;
            hunt.set("reset", f)?;
        }
        globals.set("hunt", hunt)?;

        // ── training.* ────────────────────────────────────────────────────────
        // Read-only view of the NATIVE training-mode control block (F5 / the
        // Training panel / --training). Scripts ask these instead of keeping
        // their own enforcement switches — src/training.rs owns enforcement.
        let training = lua.create_table()?;
        {
            let dbg = SharedDebugState::clone(debug);
            let f = lua.create_function(move |_, ()| -> mlua::Result<bool> {
                let ds = dbg.lock().map_err(|e| mlua::Error::external(e.to_string()))?;
                Ok(ds.training.enabled)
            })?;
            training.set("enabled", f)?;
        }
        {
            let dbg = SharedDebugState::clone(debug);
            let f = lua.create_function(move |_, ()| -> mlua::Result<bool> {
                let ds = dbg.lock().map_err(|e| mlua::Error::external(e.to_string()))?;
                Ok(ds.training.refill)
            })?;
            training.set("refill", f)?;
        }
        // dummy() -> "free"/"stand"/"crouch"/"jump"/"block" (DummyMode, lowercased).
        {
            let dbg = SharedDebugState::clone(debug);
            let f = lua.create_function(move |_, ()| -> mlua::Result<String> {
                use crate::debug::DummyMode;
                let ds = dbg.lock().map_err(|e| mlua::Error::external(e.to_string()))?;
                Ok(match ds.training.dummy {
                    DummyMode::Free => "free",
                    DummyMode::Stand => "stand",
                    DummyMode::Crouch => "crouch",
                    DummyMode::Jump => "jump",
                    DummyMode::Block => "block",
                    DummyMode::BlockPunish => "block_punish",
                }
                .to_string())
            })?;
            training.set("dummy", f)?;
        }
        {
            // The BlockPunish phase string, computed once in training::tick
            // (the panel shows the same value) — for on-screen overlays:
            //   event.onframeend(function()
            //     gui.text(4, 4, "dummy: " .. training.punish_state())
            //   end)
            let dbg = SharedDebugState::clone(debug);
            let f = lua.create_function(move |_, ()| -> mlua::Result<String> {
                let ds = dbg.lock().map_err(|e| mlua::Error::external(e.to_string()))?;
                Ok(ds.training.punish_phase.clone())
            })?;
            training.set("punish_state", f)?;
        }
        // guard_mode() -> "all"/"after_first_hit"/"random"/"none" (§9.4).
        {
            let dbg = SharedDebugState::clone(debug);
            let f = lua.create_function(move |_, ()| -> mlua::Result<String> {
                use crate::debug::GuardMode;
                let ds = dbg.lock().map_err(|e| mlua::Error::external(e.to_string()))?;
                Ok(match ds.training.guard_mode {
                    GuardMode::All => "all",
                    GuardMode::AfterFirstHit => "after_first_hit",
                    GuardMode::Random => "random",
                    GuardMode::None => "none",
                }
                .to_string())
            })?;
            training.set("guard_mode", f)?;
        }
        // Setters — the headless twin of F5/F1/the panel's pool steppers
        // (agents drive training over run_lua; hotkeys need a window). All
        // behind the ONE write gate (`--training` arms it, MCP enable_writes
        // toggles it) because they change what the app injects into the game.
        {
            let dbg = SharedDebugState::clone(debug);
            let f = lua.create_function(move |_, on: bool| -> mlua::Result<()> {
                let mut ds = dbg.lock().map_err(|e| mlua::Error::external(e.to_string()))?;
                if !ds.lua_writes_enabled {
                    return Err(mlua::Error::external(
                        "training.set_enabled blocked: writes disabled (enable_writes)",
                    ));
                }
                ds.training.enabled = on;
                if on {
                    ds.training.refill = true; // F5 parity
                }
                Ok(())
            })?;
            training.set("set_enabled", f)?;
        }
        {
            let dbg = SharedDebugState::clone(debug);
            let f = lua.create_function(move |_, name: String| -> mlua::Result<()> {
                use crate::debug::DummyMode;
                let mode = match name.as_str() {
                    "free" => DummyMode::Free,
                    "stand" => DummyMode::Stand,
                    "crouch" => DummyMode::Crouch,
                    "jump" => DummyMode::Jump,
                    "block" => DummyMode::Block,
                    "block_punish" => DummyMode::BlockPunish,
                    other => {
                        return Err(mlua::Error::external(format!("unknown dummy mode '{other}'")))
                    }
                };
                let mut ds = dbg.lock().map_err(|e| mlua::Error::external(e.to_string()))?;
                if !ds.lua_writes_enabled {
                    return Err(mlua::Error::external(
                        "training.set_dummy blocked: writes disabled (enable_writes)",
                    ));
                }
                ds.training.dummy = mode;
                Ok(())
            })?;
            training.set("set_dummy", f)?;
        }
        {
            // set_guard(mode [, percent]) — the guard-mode selector's headless
            // twin (§9.4); `percent` is Random's take-probability.
            let dbg = SharedDebugState::clone(debug);
            let f = lua.create_function(
                move |_, (name, pct): (String, Option<u8>)| -> mlua::Result<()> {
                    use crate::debug::{GuardMode, GuardPct};
                    let mode = match name.as_str() {
                        "all" => GuardMode::All,
                        "after_first_hit" => GuardMode::AfterFirstHit,
                        "random" => GuardMode::Random,
                        "none" => GuardMode::None,
                        other => {
                            return Err(mlua::Error::external(format!(
                                "unknown guard mode '{other}' (all/after_first_hit/random/none)"
                            )))
                        }
                    };
                    let mut ds = dbg.lock().map_err(|e| mlua::Error::external(e.to_string()))?;
                    if !ds.lua_writes_enabled {
                        return Err(mlua::Error::external(
                            "training.set_guard blocked: writes disabled (enable_writes)",
                        ));
                    }
                    ds.training.guard_mode = mode;
                    if let Some(p) = pct {
                        ds.training.guard_random_pct = GuardPct(p.min(100));
                    }
                    Ok(())
                },
            )?;
            training.set("set_guard", f)?;
        }
        {
            let dbg = SharedDebugState::clone(debug);
            let f = lua.create_function(move |_, pool: mlua::Table| -> mlua::Result<()> {
                use crate::macros::PunishOption;
                let mut out: Vec<(PunishOption, u8)> = Vec::new();
                for entry in pool.sequence_values::<mlua::Table>() {
                    let t = entry?;
                    let w: u8 = t.get::<Option<u8>>("weight")?.unwrap_or(1);
                    let opt = if let Some(m) = t.get::<Option<String>>("move")? {
                        PunishOption::Move(m)
                    } else if let Some(a) = t.get::<Option<String>>("attack")? {
                        PunishOption::Attack(a)
                    } else if let Some(n) = t.get::<Option<u16>>("continue_frames")? {
                        PunishOption::ContinueBlock(n)
                    } else {
                        return Err(mlua::Error::external(
                            "pool entry needs one of: move / attack / continue_frames",
                        ));
                    };
                    out.push((opt, w));
                }
                let mut ds = dbg.lock().map_err(|e| mlua::Error::external(e.to_string()))?;
                if !ds.lua_writes_enabled {
                    return Err(mlua::Error::external(
                        "training.set_punish blocked: writes disabled (enable_writes)",
                    ));
                }
                ds.training.punish_pool = out;
                Ok(())
            })?;
            training.set("set_punish", f)?;
        }
        globals.set("training", training)?;

        // ── shadow.* ──────────────────────────────────────────────────────────
        // Shadow-bot status + toggle, over the same GUI bridge fields the
        // Training panel and Shift+F5 use (drained by Frontend::drain_shadow_ops).
        let shadow = lua.create_table()?;

        // on() -> bool|nil — nil means no model is loaded (--shadow absent).
        {
            let dbg = SharedDebugState::clone(debug);
            let f = lua.create_function(move |_, ()| -> mlua::Result<Option<bool>> {
                let ds = dbg.lock().map_err(|e| mlua::Error::external(e.to_string()))?;
                Ok(ds.shadow_on)
            })?;
            shadow.set("on", f)?;
        }

        // model() -> string|nil — the loaded model's directory basename.
        {
            let dbg = SharedDebugState::clone(debug);
            let f = lua.create_function(move |_, ()| -> mlua::Result<Option<String>> {
                let ds = dbg.lock().map_err(|e| mlua::Error::external(e.to_string()))?;
                Ok(ds.shadow_model.as_ref().map(|m| m.name.clone()))
            })?;
            shadow.set("model", f)?;
        }

        // toggle() — queue a shadow on/off flip (equivalent to Shift+F5; a
        // no-op downstream when no model is loaded).
        {
            let dbg = SharedDebugState::clone(debug);
            let f = lua.create_function(move |_, ()| {
                let mut ds = dbg.lock().map_err(|e| mlua::Error::external(e.to_string()))?;
                ds.pending_shadow_toggle = true;
                Ok(())
            })?;
            shadow.set("toggle", f)?;
        }

        globals.set("shadow", shadow)?;

        // ── record.* ──────────────────────────────────────────────────────────
        // Recorder status + start/stop over the recorder GUI bridge (drained by
        // Frontend::drain_record_ops). No global-name collision: the engine's
        // other globals are memory/savestate/input/gui/event/console/emu/game/
        // training/shadow, so `record` keeps its natural name.
        let record = lua.create_table()?;

        // active() -> bool; path() -> string|nil; frames() -> integer — all
        // published per-frame by the Frontend into record_status.
        {
            let dbg = SharedDebugState::clone(debug);
            let f = lua.create_function(move |_, ()| -> mlua::Result<bool> {
                let ds = dbg.lock().map_err(|e| mlua::Error::external(e.to_string()))?;
                Ok(ds.record_status.is_some())
            })?;
            record.set("active", f)?;
        }
        {
            let dbg = SharedDebugState::clone(debug);
            let f = lua.create_function(move |_, ()| -> mlua::Result<Option<String>> {
                let ds = dbg.lock().map_err(|e| mlua::Error::external(e.to_string()))?;
                Ok(ds
                    .record_status
                    .as_ref()
                    .map(|(p, _)| p.display().to_string()))
            })?;
            record.set("path", f)?;
        }
        {
            let dbg = SharedDebugState::clone(debug);
            let f = lua.create_function(move |_, ()| -> mlua::Result<u64> {
                let ds = dbg.lock().map_err(|e| mlua::Error::external(e.to_string()))?;
                Ok(ds.record_status.as_ref().map_or(0, |(_, n)| *n))
            })?;
            record.set("frames", f)?;
        }

        // start(path [, style]) -> true — queue RecordControl::Start. Refused
        // (Lua error) while a recording is active or a start/stop is already
        // queued, mirroring the savestate "already queued" convention.
        {
            let dbg = SharedDebugState::clone(debug);
            let f = lua.create_function(
                move |_, (path, style): (String, Option<String>)| -> mlua::Result<bool> {
                    let path = path.trim().to_string();
                    if path.is_empty() {
                        return Err(mlua::Error::RuntimeError(
                            "record.start: path must be a non-empty string".to_string(),
                        ));
                    }
                    let mut ds = dbg.lock().map_err(|e| mlua::Error::external(e.to_string()))?;
                    if let Some((p, _)) = &ds.record_status {
                        return Err(mlua::Error::RuntimeError(format!(
                            "record.start: already recording to {} — record.stop() first",
                            p.display()
                        )));
                    }
                    if ds.pending_record.is_some() {
                        return Err(mlua::Error::RuntimeError(
                            "record.start: another start/stop is already queued (ops apply \
                             on the next frame drain) — retry on a later frame"
                                .to_string(),
                        ));
                    }
                    ds.pending_record = Some(crate::debug::RecordControl::Start {
                        path: std::path::PathBuf::from(path),
                        style,
                    });
                    Ok(true)
                },
            )?;
            record.set("start", f)?;
        }

        // stop() -> true — queue RecordControl::Stop (fails softly downstream
        // when nothing is recording, matching the GUI stop button).
        {
            let dbg = SharedDebugState::clone(debug);
            let f = lua.create_function(move |_, ()| -> mlua::Result<bool> {
                let mut ds = dbg.lock().map_err(|e| mlua::Error::external(e.to_string()))?;
                ds.pending_record = Some(crate::debug::RecordControl::Stop);
                Ok(true)
            })?;
            record.set("stop", f)?;
        }

        globals.set("record", record)?;

        // ── version sentinel ──────────────────────────────────────────────────
        // Scripts can check `if _RUSTRETRO_API >= 3 then … end` to feature-detect
        // (2 added memory.write*, savestate.*, input.*, gui.text; 3 added the
        // game/training/shadow/record tables, emu.paused, memory.freeze/unfreeze).
        globals.set("_RUSTRETRO_API", 3u32)?;

        Ok(())
    }

    /// Load and execute a script. `src_or_path` may be either inline Lua source or
    /// a filesystem path; if it points to an existing file the HOST reads it (Lua
    /// has no io access). Running the chunk typically calls `event.onframeend(...)`
    /// to register callbacks.
    pub fn load_script(&mut self, src_or_path: &str) -> mlua::Result<()> {
        let (src, name) = match std::fs::read_to_string(src_or_path) {
            Ok(contents) => (contents, src_or_path.to_string()),
            Err(_) => (src_or_path.to_string(), "<inline>".to_string()),
        };
        self.lua.load(&src).set_name(name).exec()
    }

    /// Execute an inline Lua chunk and return a textual result. Used by the MCP
    /// `run_lua` bridge: the script runs in the same sandboxed VM as `--script`
    /// (so `memory.*`, `gui.*`, `console.log`, `emu.*` are all available). If the
    /// chunk evaluates to a value it is stringified; otherwise "ok" is returned.
    /// Errors are returned as `Err(message)` rather than logged-and-swallowed so
    /// the caller (and ultimately the AI) sees them.
    pub fn eval_to_string(&self, src: &str) -> Result<String, String> {
        // Try as an expression first (so `2+2` or `memory.read_u8(0xFF0000)`
        // yields a value); fall back to running it as a statement chunk.
        let as_expr = format!("return ({src})");
        let result: mlua::Result<mlua::Value> = self
            .lua
            .load(&as_expr)
            .set_name("<mcp-expr>")
            .eval()
            .or_else(|_| self.lua.load(src).set_name("<mcp>").eval());
        match result {
            Ok(mlua::Value::Nil) => Ok("ok".to_string()),
            Ok(v) => match v {
                mlua::Value::String(s) => Ok(s.to_string_lossy().to_string()),
                mlua::Value::Integer(i) => Ok(i.to_string()),
                mlua::Value::Number(n) => Ok(n.to_string()),
                mlua::Value::Boolean(b) => Ok(b.to_string()),
                other => Ok(format!("{other:?}")),
            },
            Err(e) => Err(e.to_string()),
        }
    }

    /// Re-create a fresh VM, discarding all registered callbacks and draw state,
    /// then reload `src_or_path`. Use this to hot-reload a script.
    /// Called by the script panel's "Reload" and "Clear VM" buttons.
    pub fn reload(&mut self, src_or_path: &str) -> mlua::Result<()> {
        let fresh = LuaEngine::new(SharedDebugState::clone(&self.debug))?;
        *self = fresh;
        self.load_script(src_or_path)
    }

    /// Run every registered `event.onframeend` callback for this frame.
    ///
    /// Clears the draw buffer first, then invokes each callback. A Lua runtime
    /// error is caught (mlua returns `Err`), logged to the debug event log, and
    /// execution continues with the next callback — a buggy script never crashes
    /// the app. Returns `Ok(())` even when individual callbacks errored.
    pub fn run_frame_callbacks(&self) -> mlua::Result<()> {
        self.draw_cmds.borrow_mut().clear();

        // Snapshot the registry keys we need to call. We borrow the Vec only to
        // read the keys; calling back into Lua does not touch this Vec.
        let count = self.frame_callbacks.borrow().len();
        for i in 0..count {
            // Re-borrow per iteration to keep the borrow short-lived.
            let func: mlua::Function = {
                let cbs = self.frame_callbacks.borrow();
                match self.lua.registry_value(&cbs[i]) {
                    Ok(f) => f,
                    Err(e) => {
                        self.log_error(&format!("bad callback registry value: {e}"));
                        continue;
                    }
                }
            };
            if let Err(e) = func.call::<()>(()) {
                self.log_error(&format!("onframeend callback error: {e}"));
                // continue — isolate the failure.
            }
        }
        Ok(())
    }

    /// Drain the current frame's draw commands (clears the internal buffer).
    pub fn take_draw_cmds(&self) -> Vec<DrawCmd> {
        std::mem::take(&mut *self.draw_cmds.borrow_mut())
    }

    /// Clear the draw buffer without returning its contents.
    #[allow(dead_code)]
    pub fn clear_draw_cmds(&self) {
        self.draw_cmds.borrow_mut().clear();
    }

    /// Return the number of registered `event.onframeend` callbacks.
    /// Used by the script panel to show registration status.
    pub fn callback_count(&self) -> usize {
        self.frame_callbacks.borrow().len()
    }

    fn log_error(&self, msg: &str) {
        if let Ok(mut ds) = self.debug.lock() {
            ds.log(format!("[lua] ERROR: {msg}"));
        }
        eprintln!("[lua] ERROR: {msg}");
    }
}

// ─── Compositor ──────────────────────────────────────────────────────────────

/// Alpha-blend draw commands into an RGBA8888 framebuffer in GAME-PIXEL space.
///
/// `rgba` is `[R, G, B, A]` per pixel, `width × height`. Boxes get a translucent
/// fill (alpha from `fill`'s low byte) plus a solid 1px outline (`line`). Anything
/// outside the buffer is clipped. Text renders with the built-in 3×5 font.
pub fn composite_into_rgba(cmds: &[DrawCmd], rgba: &mut [u8], width: u32, height: u32) {
    let w = width as i32;
    let h = height as i32;
    if w <= 0 || h <= 0 {
        return;
    }

    for cmd in cmds {
        match *cmd {
            DrawCmd::Box {
                x1,
                y1,
                x2,
                y2,
                fill,
                line,
            } => {
                let (lx, rx) = (x1.min(x2), x1.max(x2));
                let (ty, by) = (y1.min(y2), y1.max(y2));

                let (fr, fg, fb, fa) = unpack(fill);
                // Filled interior (inclusive bounds), alpha-blended.
                if fa > 0 {
                    for py in ty..=by {
                        if py < 0 || py >= h {
                            continue;
                        }
                        for px in lx..=rx {
                            if px < 0 || px >= w {
                                continue;
                            }
                            blend_px(rgba, w, px, py, fr, fg, fb, fa);
                        }
                    }
                }

                // 1px outline (solid blend; alpha from `line`).
                let (lr, lg, lb, la) = unpack(line);
                if la > 0 {
                    // Top & bottom edges.
                    for px in lx..=rx {
                        blend_clamped(rgba, w, h, px, ty, lr, lg, lb, la);
                        blend_clamped(rgba, w, h, px, by, lr, lg, lb, la);
                    }
                    // Left & right edges.
                    for py in ty..=by {
                        blend_clamped(rgba, w, h, lx, py, lr, lg, lb, la);
                        blend_clamped(rgba, w, h, rx, py, lr, lg, lb, la);
                    }
                }
            }
            DrawCmd::Text { x, y, ref s, color, scale, shadow } => {
                if shadow {
                    // 1px down-right drop shadow: same string in black, keeping
                    // the source alpha (low byte) so translucent text shadows
                    // translucently.
                    draw_text(rgba, w, h, x + 1, y + 1, s, color & 0xFF, scale);
                }
                draw_text(rgba, w, h, x, y, s, color, scale);
            }
            DrawCmd::Line { x1, y1, x2, y2, color } => {
                let (r, g, b, a) = unpack(color);
                if a > 0 {
                    draw_line(rgba, w, h, x1, y1, x2, y2, r, g, b, a);
                }
            }
            DrawCmd::Pixel { x, y, color } => {
                let (r, g, b, a) = unpack(color);
                if a > 0 {
                    blend_clamped(rgba, w, h, x, y, r, g, b, a);
                }
            }
        }
    }
}

/// Unpack a packed `0xRRGGBBAA` color into `(r, g, b, a)`.
#[inline]
fn unpack(c: u32) -> (u8, u8, u8, u8) {
    (
        ((c >> 24) & 0xFF) as u8,
        ((c >> 16) & 0xFF) as u8,
        ((c >> 8) & 0xFF) as u8,
        (c & 0xFF) as u8,
    )
}

/// Alpha-blend a single source pixel onto `rgba` at (px, py). Caller guarantees
/// (px, py) is in-bounds. Standard src-over: out = src*a + dst*(1-a).
#[inline]
fn blend_px(rgba: &mut [u8], w: i32, px: i32, py: i32, sr: u8, sg: u8, sb: u8, sa: u8) {
    let idx = ((py * w + px) as usize) * 4;
    if idx + 3 >= rgba.len() {
        return;
    }
    let a = sa as u32;
    let inv = 255 - a;
    rgba[idx] = ((sr as u32 * a + rgba[idx] as u32 * inv) / 255) as u8;
    rgba[idx + 1] = ((sg as u32 * a + rgba[idx + 1] as u32 * inv) / 255) as u8;
    rgba[idx + 2] = ((sb as u32 * a + rgba[idx + 2] as u32 * inv) / 255) as u8;
    // Keep framebuffer opaque.
    rgba[idx + 3] = 0xFF;
}

/// Bounds-checked variant of `blend_px`.
#[inline]
fn blend_clamped(rgba: &mut [u8], w: i32, h: i32, px: i32, py: i32, sr: u8, sg: u8, sb: u8, sa: u8) {
    if px < 0 || px >= w || py < 0 || py >= h {
        return;
    }
    blend_px(rgba, w, px, py, sr, sg, sb, sa);
}

/// Bresenham integer line rasteriser. Draws all pixels from (x1,y1) to (x2,y2)
/// inclusive, alpha-blending each. Out-of-bounds pixels are silently clipped.
fn draw_line(rgba: &mut [u8], w: i32, h: i32, x1: i32, y1: i32, x2: i32, y2: i32, r: u8, g: u8, b: u8, a: u8) {
    let mut x = x1;
    let mut y = y1;
    let dx = (x2 - x1).abs();
    let dy = (y2 - y1).abs();
    let sx: i32 = if x1 < x2 { 1 } else { -1 };
    let sy: i32 = if y1 < y2 { 1 } else { -1 };
    let mut err = dx - dy;

    loop {
        blend_clamped(rgba, w, h, x, y, r, g, b, a);
        if x == x2 && y == y2 {
            break;
        }
        let e2 = err * 2;
        if e2 > -dy {
            err -= dy;
            x += sx;
        }
        if e2 < dx {
            err += dx;
            y += sy;
        }
    }
}

/// Minimal blocky text renderer: draws each character as a small filled 3×5 dot
/// pattern using a tiny built-in font. Good enough to label hitboxes; not a full
/// typeface. Unsupported glyphs render as a solid block.
fn draw_text(rgba: &mut [u8], w: i32, h: i32, x: i32, y: i32, s: &str, color: u32, scale: i32) {
    let (r, g, b, a) = unpack(color);
    if a == 0 {
        return;
    }
    let scale = scale.max(1);
    const GW: i32 = 3; // glyph width in font pixels
    const CW: i32 = 4; // cell width in font pixels (3px glyph + 1px gap)
    let mut cx = x;
    for ch in s.chars() {
        let rows = glyph(ch);
        for (ry, bits) in rows.iter().enumerate() {
            for rxi in 0..GW {
                if (bits >> (GW - 1 - rxi)) & 1 == 1 {
                    // Render each font pixel as a scale×scale block.
                    for sy in 0..scale {
                        for sx in 0..scale {
                            let px = cx + rxi * scale + sx;
                            let py = y + ry as i32 * scale + sy;
                            blend_clamped(rgba, w, h, px, py, r, g, b, a);
                        }
                    }
                }
            }
        }
        cx += CW * scale;
        if cx >= w {
            break;
        }
    }
}

/// 3×5 bitmap font: each glyph is 5 rows, each row is 3 low bits (MSB = left).
/// Covers 0-9, the FULL A-Z (uppercased), space, and common symbols; falls back
/// to a solid block only for genuinely unknown glyphs. The full alphabet matters
/// for overlay labels — "NEUTRAL", "STARTUP", "ACTIVE", "RECOVERY" all use
/// letters (N/U/V/W/M/K/etc.) that an incomplete font would render as blocks.
fn glyph(ch: char) -> [u8; 5] {
    let c = ch.to_ascii_uppercase();
    match c {
        ' ' => [0b000, 0b000, 0b000, 0b000, 0b000],
        '0' => [0b111, 0b101, 0b101, 0b101, 0b111],
        '1' => [0b010, 0b110, 0b010, 0b010, 0b111],
        '2' => [0b111, 0b001, 0b111, 0b100, 0b111],
        '3' => [0b111, 0b001, 0b111, 0b001, 0b111],
        '4' => [0b101, 0b101, 0b111, 0b001, 0b001],
        '5' => [0b111, 0b100, 0b111, 0b001, 0b111],
        '6' => [0b111, 0b100, 0b111, 0b101, 0b111],
        '7' => [0b111, 0b001, 0b010, 0b010, 0b010],
        '8' => [0b111, 0b101, 0b111, 0b101, 0b111],
        '9' => [0b111, 0b101, 0b111, 0b001, 0b111],
        'A' => [0b111, 0b101, 0b111, 0b101, 0b101],
        'B' => [0b110, 0b101, 0b110, 0b101, 0b110],
        'C' => [0b111, 0b100, 0b100, 0b100, 0b111],
        'D' => [0b110, 0b101, 0b101, 0b101, 0b110],
        'E' => [0b111, 0b100, 0b110, 0b100, 0b111],
        'F' => [0b111, 0b100, 0b110, 0b100, 0b100],
        'G' => [0b111, 0b100, 0b101, 0b101, 0b111],
        'H' => [0b101, 0b101, 0b111, 0b101, 0b101],
        'I' => [0b111, 0b010, 0b010, 0b010, 0b111],
        'J' => [0b001, 0b001, 0b001, 0b101, 0b111],
        'K' => [0b101, 0b101, 0b110, 0b101, 0b101],
        'L' => [0b100, 0b100, 0b100, 0b100, 0b111],
        'M' => [0b101, 0b111, 0b111, 0b101, 0b101],
        'N' => [0b101, 0b111, 0b111, 0b111, 0b101],
        'O' => [0b111, 0b101, 0b101, 0b101, 0b111],
        'P' => [0b111, 0b101, 0b111, 0b100, 0b100],
        'Q' => [0b111, 0b101, 0b101, 0b111, 0b001],
        'R' => [0b110, 0b101, 0b110, 0b101, 0b101],
        'S' => [0b111, 0b100, 0b111, 0b001, 0b111],
        'T' => [0b111, 0b010, 0b010, 0b010, 0b010],
        'U' => [0b101, 0b101, 0b101, 0b101, 0b111],
        'V' => [0b101, 0b101, 0b101, 0b101, 0b010],
        'W' => [0b101, 0b101, 0b111, 0b111, 0b101],
        'X' => [0b101, 0b101, 0b010, 0b101, 0b101],
        'Y' => [0b101, 0b101, 0b010, 0b010, 0b010],
        'Z' => [0b111, 0b001, 0b010, 0b100, 0b111],
        ':' => [0b000, 0b010, 0b000, 0b010, 0b000],
        '-' => [0b000, 0b000, 0b111, 0b000, 0b000],
        '+' => [0b000, 0b010, 0b111, 0b010, 0b000],
        '.' => [0b000, 0b000, 0b000, 0b000, 0b010],
        ',' => [0b000, 0b000, 0b000, 0b010, 0b100],
        '/' => [0b001, 0b001, 0b010, 0b100, 0b100],
        '%' => [0b101, 0b001, 0b010, 0b100, 0b101],
        '(' => [0b001, 0b010, 0b010, 0b010, 0b001],
        ')' => [0b100, 0b010, 0b010, 0b010, 0b100],
        '!' => [0b010, 0b010, 0b010, 0b000, 0b010],
        '#' => [0b101, 0b111, 0b101, 0b111, 0b101],
        _ => [0b111, 0b111, 0b111, 0b111, 0b111], // unknown → solid block
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::debug::{BusWindowCfg, DebugState, StateOp};
    use std::sync::{Arc, Mutex};

    const BLOCK: [u8; 5] = [0b111, 0b111, 0b111, 0b111, 0b111];

    /// A LuaEngine over a DebugState with one writable Work-RAM bus window at
    /// 0x400000 (owned snapshot buffer + pending_bus_writes queue — no core).
    fn engine_with_ram() -> (LuaEngine, SharedDebugState) {
        let dbg: SharedDebugState = Arc::new(Mutex::new(DebugState::new()));
        assert!(dbg.lock().unwrap().install_bus_window(BusWindowCfg {
            name: "Work RAM".into(),
            addr: 0x400000,
            len: 0x1000,
            interval: 1,
            flags: crate::libretro::RETRO_MEMDESC_SYSTEM_RAM,
        }));
        let eng = LuaEngine::new(SharedDebugState::clone(&dbg)).unwrap();
        (eng, dbg)
    }

    #[test]
    fn write_gate_blocks_then_allows() {
        let (eng, dbg) = engine_with_ram();
        // Gate off (default): the write raises an error NAMING the gate.
        let err = eng
            .eval_to_string("memory.writebyte(0x400010, 0xAB)")
            .unwrap_err();
        assert!(err.contains("lua_writes_enabled"), "error must name the gate: {err}");
        assert!(dbg.lock().unwrap().pending_bus_writes.is_empty());

        // Armed: the write lands in the snapshot AND the live-bus queue.
        dbg.lock().unwrap().lua_writes_enabled = true;
        eng.eval_to_string("memory.writebyte(0x400010, 0xAB)").unwrap();
        {
            let ds = dbg.lock().unwrap();
            assert_eq!(ds.read_u8(0x400010), Some(0xAB));
            assert_eq!(ds.pending_bus_writes, vec![(0x400010, vec![0xAB])]);
        }
        // Out-of-map address: distinct error, nothing queued.
        let err = eng
            .eval_to_string("memory.writebyte(0xDEAD0000, 1)")
            .unwrap_err();
        assert!(err.contains("no writable region"), "{err}");
    }

    #[test]
    fn writeword_is_guest_big_endian() {
        let (eng, dbg) = engine_with_ram();
        dbg.lock().unwrap().lua_writes_enabled = true;
        eng.eval_to_string("memory.writeword(0x400020, 0x1234)").unwrap();
        {
            let ds = dbg.lock().unwrap();
            // High byte at the lower address — guest (68k) big-endian order.
            assert_eq!(ds.read_u8(0x400020), Some(0x12));
            assert_eq!(ds.read_u8(0x400021), Some(0x34));
            // The live-bus queue carries the same ascending-address bytes.
            assert_eq!(ds.pending_bus_writes, vec![(0x400020, vec![0x12, 0x34])]);
        }
        // Round-trips through the matching BE read.
        assert_eq!(
            eng.eval_to_string("memory.read_u16_be(0x400020)").unwrap(),
            "4660" // 0x1234
        );
    }

    #[test]
    fn savestate_enqueues_slots_and_paths_and_refuses_in_flight() {
        let (eng, dbg) = engine_with_ram();
        // Slot save enqueues; returns true.
        assert_eq!(eng.eval_to_string("savestate.save(3)").unwrap(), "true");
        assert_eq!(
            dbg.lock().unwrap().pending_state_op,
            Some(StateOp::SaveSlot(3))
        );
        // A second op while one is queued is an error, and the queue is untouched.
        let err = eng.eval_to_string("savestate.load(2)").unwrap_err();
        assert!(err.contains("already queued"), "{err}");
        assert_eq!(
            dbg.lock().unwrap().pending_state_op.take(),
            Some(StateOp::SaveSlot(3))
        );
        // Path form (string) → Load(path).
        assert_eq!(
            eng.eval_to_string("savestate.load('/tmp/rr_test.state')").unwrap(),
            "true"
        );
        assert_eq!(
            dbg.lock().unwrap().pending_state_op.take(),
            Some(StateOp::Load(std::path::PathBuf::from("/tmp/rr_test.state")))
        );
        // Bad slots / types are errors and enqueue nothing.
        for bad in ["savestate.save(0)", "savestate.save(10)", "savestate.save(true)"] {
            assert!(eng.eval_to_string(bad).is_err(), "{bad} should fail");
        }
        assert!(dbg.lock().unwrap().pending_state_op.is_none());
    }

    #[test]
    fn buttons_mask_and_pairs_parse() {
        // Mask decode: bit i = RETRO id i.
        let b = buttons_from_mask(0b0000_1001_0000).unwrap(); // Up(4) + Right(7)
        assert!(b[4] && b[7]);
        assert_eq!(b.iter().filter(|x| **x).count(), 2);
        assert_eq!(mask_from_buttons(&b), 0b0000_1001_0000);
        // Out-of-range masks refused.
        assert!(buttons_from_mask(-1).is_err());
        assert!(buttons_from_mask(0x1000).is_err());
        // Name pairs (case-insensitive, shared with MCP press_buttons).
        let b = buttons_from_pairs(&[("Right".into(), true), ("b".into(), true)]).unwrap();
        assert!(b[7] && b[0]);
        // Unknown names fail loudly.
        let err = buttons_from_pairs(&[("bb".into(), true)]).unwrap_err();
        assert!(err.contains("unknown button 'bb'"), "{err}");
    }

    #[test]
    fn input_set_writes_hold_counters_and_validates() {
        let (eng, dbg) = engine_with_ram();
        // Table form on port 1 → 2-frame holds on injected_input2 only.
        eng.eval_to_string("input.set(1, {right=true, b=true})").unwrap();
        {
            let ds = dbg.lock().unwrap();
            assert_eq!(ds.injected_input2[7], 2);
            assert_eq!(ds.injected_input2[0], 2);
            assert_eq!(ds.injected_input2.iter().filter(|c| **c > 0).count(), 2);
            assert_eq!(ds.injected_input, [0u16; 12], "P1 untouched");
        }
        // Mask form on port 0; a later set(0, 0) releases everything.
        eng.eval_to_string("input.set(0, 0x30)").unwrap(); // Up+Down
        assert_eq!(dbg.lock().unwrap().injected_input[4], 2);
        assert_eq!(dbg.lock().unwrap().injected_input[5], 2);
        eng.eval_to_string("input.set(0, 0)").unwrap();
        assert_eq!(dbg.lock().unwrap().injected_input, [0u16; 12]);
        // Bad port / bad button / bad type all raise.
        assert!(eng.eval_to_string("input.set(2, 0)").is_err());
        assert!(eng.eval_to_string("input.set(0, {zz=true})").is_err());
        assert!(eng.eval_to_string("input.set(0, 'up')").is_err());
    }

    #[test]
    fn input_get_returns_mirrored_masks() {
        let (eng, dbg) = engine_with_ram();
        {
            let mut ds = dbg.lock().unwrap();
            ds.input_state[7] = true; // P1 Right
            ds.input_state2[0] = true; // P2 B
        }
        assert_eq!(eng.eval_to_string("input.get(0)").unwrap(), "128");
        assert_eq!(eng.eval_to_string("input.get(1)").unwrap(), "1");
        assert!(eng.eval_to_string("input.get(2)").is_err());
    }

    #[test]
    fn input_hold_release_round_trip_through_get() {
        let (eng, dbg) = engine_with_ram();
        eng.eval_to_string("input.hold(0, {right=true})").unwrap();
        // Simulate the run loop's fold (main.rs/read_input): take_injected_input
        // folds held+countdown into the controller bitmap, then the frontend
        // mirrors that into input_state, which input.get reads.
        let fold = |dbg: &crate::debug::SharedDebugState| {
            let mut ds = dbg.lock().unwrap();
            ds.input_state = ds.take_injected_input();
        };
        fold(&dbg);
        assert_eq!(eng.eval_to_string("input.get(0)").unwrap(), "128"); // Right bit
        // Held survives MANY folds with no decay, unlike input.set's countdown.
        for _ in 0..20 {
            let mut ds = dbg.lock().unwrap();
            assert!(ds.take_injected_input()[7]);
        }
        // Table-form release clears just `right`.
        eng.eval_to_string("input.release(0, {right=true})").unwrap();
        fold(&dbg);
        assert_eq!(eng.eval_to_string("input.get(0)").unwrap(), "0");
        // Bare release (no second arg) clears the whole held set.
        eng.eval_to_string("input.hold(0, 0x90)").unwrap(); // Up+Right
        eng.eval_to_string("input.release(0)").unwrap();
        fold(&dbg);
        assert_eq!(eng.eval_to_string("input.get(0)").unwrap(), "0");
        // Port 1 independent of port 0; bad port rejected on both.
        eng.eval_to_string("input.hold(1, {b=true})").unwrap();
        {
            let mut ds = dbg.lock().unwrap();
            assert!(ds.take_injected_input2()[0]);
            assert!(!ds.take_injected_input()[0]);
        }
        assert!(eng.eval_to_string("input.hold(2, 0)").is_err());
        assert!(eng.eval_to_string("input.release(2)").is_err());
    }

    #[test]
    fn gui_text_pushes_shadowed_command() {
        let (eng, _dbg) = engine_with_ram();
        eng.eval_to_string("gui.text(2, 3, 'HI', 0x00FF00FF)").unwrap();
        eng.eval_to_string("gui.drawText(4, 5, 'YO')").unwrap();
        let cmds = eng.take_draw_cmds();
        assert_eq!(cmds.len(), 2);
        assert!(matches!(
            &cmds[0],
            DrawCmd::Text { x: 2, y: 3, s, color: 0x00FF00FF, shadow: true, .. } if s == "HI"
        ));
        assert!(matches!(
            &cmds[1],
            DrawCmd::Text { shadow: false, .. }
        ));
    }

    /// Count font pixels (set bits) in a glyph.
    fn glyph_popcount(g: &[u8; 5]) -> u32 {
        g.iter().map(|row| (row & 0b111).count_ones()).sum()
    }

    /// Count pixels whose red channel is exactly `v` in an RGBA buffer.
    fn count_red(rgba: &[u8], v: u8) -> usize {
        rgba.chunks_exact(4).filter(|p| p[0] == v).count()
    }

    #[test]
    fn font_covers_full_alphabet_and_digits() {
        // The whole point of completing the font: no A-Z / 0-9 glyph may fall
        // through to the solid-block fallback (which would render as a blob).
        for ch in ('A'..='Z').chain('0'..='9') {
            assert_ne!(
                glyph(ch),
                BLOCK,
                "glyph '{ch}' is missing (renders as a solid block)"
            );
            // Lowercase maps to the same glyph (uppercased internally).
            if ch.is_ascii_alphabetic() {
                assert_eq!(glyph(ch), glyph(ch.to_ascii_lowercase()));
            }
        }
        // The fighting-game category words must be fully renderable.
        for word in ["NEUTRAL", "STARTUP", "ACTIVE", "RECOVERY", "STUN", "WHIFF"] {
            for ch in word.chars() {
                assert_ne!(glyph(ch), BLOCK, "'{ch}' in {word:?} missing");
            }
        }
        // A genuinely unknown glyph still falls back to the block.
        assert_eq!(glyph('\u{2603}'), BLOCK); // snowman ☃
    }

    #[test]
    fn draw_text_scale_magnifies_each_font_pixel() {
        // '1' has a known popcount; at scale N every font pixel becomes N×N.
        let g = glyph('1');
        let pop = glyph_popcount(&g) as usize;
        let (w, h) = (64i32, 32i32);
        for scale in [1, 2, 3] {
            let mut rgba = vec![0u8; (w * h * 4) as usize];
            draw_text(&mut rgba, w, h, 2, 2, "1", 0xFFFF_FFFF, scale);
            let lit = count_red(&rgba, 0xFF);
            assert_eq!(
                lit,
                pop * (scale * scale) as usize,
                "scale {scale}: expected {} lit px",
                pop * (scale * scale) as usize
            );
        }
    }

    #[test]
    fn draw_text_zero_alpha_is_noop_and_advances_cells() {
        let (w, h) = (64i32, 16i32);
        // Fully transparent → draws nothing.
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        draw_text(&mut rgba, w, h, 0, 0, "ABC", 0xFFFF_FF00, 1);
        assert_eq!(count_red(&rgba, 0xFF), 0);

        // Two chars at scale 1 occupy distinct 4px cells → the second glyph's
        // pixels start at x>=4 (cell width). Draw "I I" and confirm pixels exist
        // past the first cell.
        let mut rgba2 = vec![0u8; (w * h * 4) as usize];
        draw_text(&mut rgba2, w, h, 0, 0, "II", 0xFFFF_FFFF, 1);
        let far = rgba2
            .chunks_exact(4)
            .enumerate()
            .any(|(i, p)| p[0] == 0xFF && (i as i32 % w) >= 4);
        assert!(far, "second glyph should render in the next cell (x>=4)");
    }

    #[test]
    fn composite_text_command_renders_at_scale() {
        let (w, h) = (64u32, 32u32);
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        let cmds = vec![DrawCmd::Text {
            x: 1,
            y: 1,
            s: "8".to_string(),
            color: 0xFFFF_FFFF,
            scale: 2,
            shadow: false,
        }];
        composite_into_rgba(&cmds, &mut rgba, w, h);
        let pop = glyph_popcount(&glyph('8')) as usize;
        assert_eq!(count_red(&rgba, 0xFF), pop * 4);
    }

    #[test]
    fn text_shadow_darkens_offset_pixels() {
        // Mid-gray background so a BLACK shadow is measurable (on black it
        // would be invisible). shadow=true must produce (a) full-white glyph
        // pixels and (b) some pixels DARKER than the background at the +1,+1
        // offset; shadow=false must produce no darkened pixels at all.
        let (w, h) = (64u32, 32u32);
        let render = |shadow: bool| {
            let mut rgba = vec![0x80u8; (w * h * 4) as usize];
            let cmds = vec![DrawCmd::Text {
                x: 4,
                y: 4,
                s: "I".to_string(),
                color: 0xFFFF_FFFF,
                scale: 1,
                shadow,
            }];
            composite_into_rgba(&cmds, &mut rgba, w, h);
            rgba
        };
        let with = render(true);
        let without = render(false);
        let dark = |buf: &[u8]| buf.chunks_exact(4).filter(|p| p[0] < 0x80).count();
        let pop = glyph_popcount(&glyph('I')) as usize;
        assert_eq!(count_red(&with, 0xFF), pop, "glyph must render over shadow");
        assert!(dark(&with) > 0, "shadow must darken offset pixels");
        assert_eq!(dark(&without), 0, "drawText (no shadow) must not darken");
    }

    /// Engine + DebugState with a bus window wide enough to cover BOTH
    /// asurabld fighter blocks and every gate global (0x400000..0x407000) and
    /// the asurabld profile installed — the fixture for the game.* tests,
    /// mirroring record.rs's `recorder_emits_round_summaries_to_the_rounds_sidecar`.
    fn engine_with_profile_and_wram() -> (LuaEngine, SharedDebugState) {
        crate::profile::init_for_tests();
        let dbg: SharedDebugState = Arc::new(Mutex::new(DebugState::new()));
        assert!(dbg.lock().unwrap().install_bus_window(BusWindowCfg {
            name: "wram-test".into(),
            addr: 0x400000,
            len: 0x7000,
            interval: 1,
            flags: crate::libretro::RETRO_MEMDESC_SYSTEM_RAM,
        }));
        let eng = LuaEngine::new(SharedDebugState::clone(&dbg)).unwrap();
        (eng, dbg)
    }

    #[test]
    fn game_controllable_flips_exactly_like_the_recorder_gate() {
        let (eng, dbg) = engine_with_profile_and_wram();
        let p = crate::profile::current();
        let (health_off, _) = p.field_off("health").unwrap();
        let timer = p.global("round_timer").unwrap() as usize;
        let char_sel = p.global("char_select").unwrap() as usize;

        // Fresh RAM: healths are 0 and the timer is 0 → gate CLOSED (the same
        // state record.rs's recorder_writes_valid_jsonl test asserts closed).
        assert_eq!(eng.eval_to_string("game.controllable()").unwrap(), "false");

        // Open it exactly the way the recorder test opens the v3 gate: live
        // healths + valid BCD clock (hop flags/char_select already 0).
        {
            let mut ds = dbg.lock().unwrap();
            assert!(ds.write_addr((p.block1() + health_off) as usize, 1, 0xEF));
            assert!(ds.write_addr((p.block2() + health_off) as usize, 1, 0xEF));
            assert!(ds.write_addr(timer, 1, 0x90));
        }
        assert_eq!(eng.eval_to_string("game.controllable()").unwrap(), "true");

        // Gate v3: a running char-select countdown closes it...
        assert!(dbg.lock().unwrap().write_addr(char_sel, 1, 0x21));
        assert_eq!(eng.eval_to_string("game.controllable()").unwrap(), "false");
        // ...and its end re-opens it.
        assert!(dbg.lock().unwrap().write_addr(char_sel, 1, 0));
        assert_eq!(eng.eval_to_string("game.controllable()").unwrap(), "true");

        // Menu-garbage clock (non-BCD) closes it, like timer_bcd_valid(0xFF).
        assert!(dbg.lock().unwrap().write_addr(timer, 1, 0xFF));
        assert_eq!(eng.eval_to_string("game.controllable()").unwrap(), "false");
        assert!(dbg.lock().unwrap().write_addr(timer, 1, 0x85));
        assert_eq!(eng.eval_to_string("game.controllable()").unwrap(), "true");

        // A latched hop flag (word_zero condition) closes it.
        let round_over = p.global("round_over").unwrap() as usize;
        assert!(dbg.lock().unwrap().write_addr(round_over, 2, 0x0100)); // guest BE 0x0001
        assert_eq!(eng.eval_to_string("game.controllable()").unwrap(), "false");

        // A KO'd health (outside 1..=0xEF) closes it too.
        assert!(dbg.lock().unwrap().write_addr(round_over, 2, 0));
        assert!(dbg
            .lock()
            .unwrap()
            .write_addr((p.block2() + health_off) as usize, 1, 0));
        assert_eq!(eng.eval_to_string("game.controllable()").unwrap(), "false");
    }

    #[test]
    fn game_profile_bindings_resolve_names_not_addresses() {
        let (eng, _dbg) = engine_with_profile_and_wram();
        let ck = |src: &str, want: &str| {
            assert_eq!(eng.eval_to_string(src).unwrap(), want, "{src}");
        };
        ck("game.addr('credits')", &format!("{}", 0x40655D));
        ck("game.addr('round_timer')", &format!("{}", 0x40000A));
        ck("game.addr('no_such_global') == nil", "true");
        ck("game.block1()", &format!("{}", 0x403798));
        ck("game.block2()", &format!("{}", 0x40454C));
        ck("game.field_off('health')", &format!("{}", 0x177));
        ck("game.field_off('char_id')", &format!("{}", 0x639));
        ck("game.field_off('no_such_field') == nil", "true");
        ck("game.char_name(7)", "rosemary");
        ck("game.char_name(1)", "goat");
        ck("game.char_name(11)", "c11");
        ck("game.matchup_slug(1, 7)", "goat-vs-rosemary");
        ck("game.stage_value_for(7)", "5");
        ck("game.stage_value_for(3) == nil", "true"); // footee has no selector
        ck("game.calibration('GROUND_Y')", "216");
        ck("game.calibration('no_such_key') == nil", "true");
    }

    #[test]
    fn training_shadow_and_paused_bindings_mirror_debug_state() {
        let (eng, dbg) = engine_with_ram();
        // Defaults: training off/free, no shadow model, not paused.
        assert_eq!(eng.eval_to_string("training.enabled()").unwrap(), "false");
        assert_eq!(eng.eval_to_string("training.refill()").unwrap(), "false");
        assert_eq!(eng.eval_to_string("training.dummy()").unwrap(), "free");
        assert_eq!(eng.eval_to_string("shadow.on() == nil").unwrap(), "true");
        assert_eq!(eng.eval_to_string("shadow.model() == nil").unwrap(), "true");
        assert_eq!(eng.eval_to_string("emu.paused()").unwrap(), "false");

        {
            let mut ds = dbg.lock().unwrap();
            ds.training.enabled = true;
            ds.training.refill = true;
            ds.training.dummy = crate::debug::DummyMode::Crouch;
            ds.shadow_on = Some(false);
            ds.shadow_model = Some(crate::debug::ShadowModelInfo {
                name: "goat-v2".into(),
                ..Default::default()
            });
            ds.paused = true;
        }
        assert_eq!(eng.eval_to_string("training.enabled()").unwrap(), "true");
        assert_eq!(eng.eval_to_string("training.refill()").unwrap(), "true");
        assert_eq!(eng.eval_to_string("training.dummy()").unwrap(), "crouch");
        assert_eq!(eng.eval_to_string("shadow.on()").unwrap(), "false");
        assert_eq!(eng.eval_to_string("shadow.model()").unwrap(), "goat-v2");
        assert_eq!(eng.eval_to_string("emu.paused()").unwrap(), "true");

        // toggle() queues the one-shot the Frontend drains (Shift+F5 twin).
        assert!(!dbg.lock().unwrap().pending_shadow_toggle);
        eng.eval_to_string("shadow.toggle()").unwrap();
        assert!(dbg.lock().unwrap().pending_shadow_toggle);
    }

    #[test]
    fn record_bindings_report_queue_and_refuse() {
        use crate::debug::RecordControl;
        let (eng, dbg) = engine_with_ram();
        // Idle: no recording, zero frames, nil path.
        assert_eq!(eng.eval_to_string("record.active()").unwrap(), "false");
        assert_eq!(eng.eval_to_string("record.frames()").unwrap(), "0");
        assert_eq!(eng.eval_to_string("record.path() == nil").unwrap(), "true");

        // start(path, style) queues a Start with both fields.
        assert_eq!(
            eng.eval_to_string("record.start('/tmp/rr_rec.jsonl', 'rushdown')")
                .unwrap(),
            "true"
        );
        assert_eq!(
            dbg.lock().unwrap().pending_record,
            Some(RecordControl::Start {
                path: std::path::PathBuf::from("/tmp/rr_rec.jsonl"),
                style: Some("rushdown".into()),
            })
        );
        // A second start while one is queued is refused; the queue is untouched.
        let err = eng
            .eval_to_string("record.start('/tmp/other.jsonl')")
            .unwrap_err();
        assert!(err.contains("already queued"), "{err}");
        dbg.lock().unwrap().pending_record = None;

        // With a live recording published, status reads through and start refuses.
        dbg.lock().unwrap().record_status =
            Some((std::path::PathBuf::from("/tmp/rr_rec.jsonl"), 42));
        assert_eq!(eng.eval_to_string("record.active()").unwrap(), "true");
        assert_eq!(eng.eval_to_string("record.frames()").unwrap(), "42");
        assert_eq!(
            eng.eval_to_string("record.path()").unwrap(),
            "/tmp/rr_rec.jsonl"
        );
        let err = eng
            .eval_to_string("record.start('/tmp/other.jsonl')")
            .unwrap_err();
        assert!(err.contains("already recording"), "{err}");

        // stop() queues Stop; style-less start parses (style = None).
        assert_eq!(eng.eval_to_string("record.stop()").unwrap(), "true");
        assert_eq!(
            dbg.lock().unwrap().pending_record.take(),
            Some(RecordControl::Stop)
        );
        dbg.lock().unwrap().record_status = None;
        eng.eval_to_string("record.start('/tmp/plain.jsonl')").unwrap();
        assert!(matches!(
            dbg.lock().unwrap().pending_record.take(),
            Some(RecordControl::Start { style: None, .. })
        ));
        // Empty path refused.
        assert!(eng.eval_to_string("record.start('  ')").is_err());
    }

    #[test]
    fn memory_freeze_unfreeze_gated_and_manage_frozen_watches() {
        let (eng, dbg) = engine_with_ram();
        // Both are gated exactly like writebyte: the error names the gate.
        for src in ["memory.freeze(0x400010, 0xAB)", "memory.unfreeze(0x400010)"] {
            let err = eng.eval_to_string(src).unwrap_err();
            assert!(err.contains("lua_writes_enabled"), "{src}: {err}");
        }
        assert!(dbg.lock().unwrap().watches.is_empty());

        dbg.lock().unwrap().lua_writes_enabled = true;
        eng.eval_to_string("memory.freeze(0x400010, 0xAB)").unwrap();
        {
            let ds = dbg.lock().unwrap();
            assert_eq!(ds.watches.len(), 1);
            let w = &ds.watches[0];
            assert_eq!(w.addr, 0x400010);
            assert!(w.frozen);
            assert_eq!(w.frozen_value, Some(0xAB));
            assert_eq!(w.label, "lua freeze");
            assert!(matches!(w.format, crate::debug::WatchFormat::Hex8));
            assert!(!w.track_changes);
        }
        // Re-freezing the same addr REPLACES (no duplicate frozen watches).
        eng.eval_to_string("memory.freeze(0x400010, 0xCD)").unwrap();
        {
            let ds = dbg.lock().unwrap();
            assert_eq!(ds.watches.len(), 1);
            assert_eq!(ds.watches[0].frozen_value, Some(0xCD));
        }
        // A NON-frozen watch at the same addr survives unfreeze.
        dbg.lock().unwrap().watches.push(crate::debug::Watch {
            addr: 0x400010,
            label: "plain watch".into(),
            format: crate::debug::WatchFormat::U8,
            frozen: false,
            frozen_value: None,
            track_changes: false,
            current: None,
            prev_value: None,
        });
        eng.eval_to_string("memory.unfreeze(0x400010)").unwrap();
        {
            let ds = dbg.lock().unwrap();
            assert_eq!(ds.watches.len(), 1);
            assert_eq!(ds.watches[0].label, "plain watch");
        }
    }

    #[test]
    fn api_sentinel_is_v3() {
        let (eng, _dbg) = engine_with_ram();
        assert_eq!(eng.eval_to_string("_RUSTRETRO_API").unwrap(), "3");
    }

    #[test]
    fn composite_box_fill_and_outline_blend() {
        let (w, h) = (16u32, 16u32);
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        // Opaque red fill, opaque green outline.
        let cmds = vec![DrawCmd::Box {
            x1: 2,
            y1: 2,
            x2: 6,
            y2: 6,
            fill: 0xFF00_00FF,
            line: 0x00FF_00FF,
        }];
        composite_into_rgba(&cmds, &mut rgba, w, h);
        // A corner pixel (2,2) is on the outline → green.
        let corner = ((2 * w + 2) * 4) as usize;
        assert_eq!(&rgba[corner..corner + 3], &[0x00, 0xFF, 0x00]);
        // An interior pixel (4,4) is fill → red.
        let inner = ((4 * w + 4) * 4) as usize;
        assert_eq!(&rgba[inner..inner + 3], &[0xFF, 0x00, 0x00]);
        // Framebuffer alpha stays opaque.
        assert_eq!(rgba[inner + 3], 0xFF);
    }
}
